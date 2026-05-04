use std::collections::HashMap;

use crate::match_engine::{MatchEngine, MatchEngineError, MatchResult};
use crate::types::{
    Account, AccountError, AccountId, Amount, AssetId, Order, OrderBookSide, OrderId, OrderKind,
    STABLE_ASSET_ID, Side, Tif,
};

/// Things that happened to one or more orders while processing a submission (append-only log).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderEvent {
    /// A new resting limit was inserted into the book (same `order_id` may appear again after a
    /// partial fill that leaves a GTC remainder).
    Added { order_id: OrderId },
    /// One aggressive step: `taker_order_id` vs this `maker_order_id` with the engine fill detail.
    Matched {
        taker_order_id: OrderId,
        maker_order_id: OrderId,
        match_result: MatchResult,
    },
    /// A resting order was removed by user cancel; collateral for `order_id` was refunded.
    Cancelled { order_id: OrderId },
    /// A resting order id is fully done (fully filled as maker, or taker fully consumed with no
    /// remainder that will rest).
    Filled { order_id: OrderId },
}

/// Errors surfaced when routing an order through accounts and the matching engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderBookError {
    Account(AccountError),
    AccountNotFound,
    MarketOrderCannotRest,
    MatchEngine(MatchEngineError),
    OrderNotFound,
    OrderOwnerMismatch,
    InvalidAsset,
    OrderAmountTooSmall,
}

/// In-memory central limit order book: resting bids/asks, monotonic order ids, per-account
/// balances, and the match engine used to price/aggress incoming orders.
#[derive(Debug)]
pub struct OrderBook {
    next_order_id: OrderId,
    orders: HashMap<OrderId, Order>,
    pub buys: OrderBookSide,
    pub sells: OrderBookSide,
    match_engine: MatchEngine,
    accounts: HashMap<AccountId, Account>,
}

impl OrderBook {
    /// Builds an empty book and match engine; `supported_asset` lists non-stable symbols that may
    /// trade against the configured stable collateral asset.
    pub fn new(supported_asset: impl IntoIterator<Item = AssetId>) -> Self {
        Self {
            next_order_id: 1,
            orders: HashMap::new(),
            buys: OrderBookSide::new(Side::Buy),
            sells: OrderBookSide::new(Side::Sell),
            match_engine: MatchEngine::new(supported_asset),
            accounts: HashMap::new(),
        }
    }

    pub fn add_account(&mut self, account: Account) {
        self.accounts.insert(account.id(), account);
    }

    pub fn account(&self, account_id: AccountId) -> Option<&Account> {
        self.accounts.get(&account_id)
    }

    pub fn accounts(&self) -> &HashMap<AccountId, Account> {
        &self.accounts
    }

    pub fn orders(&self) -> &HashMap<OrderId, Order> {
        &self.orders
    }

    /// Validates balance, debits full order collateral into the account (no separate escrow
    /// bucket), allocates an id, then routes the order through `process_order`.
    pub fn place_order(
        &mut self,
        account_id: AccountId,
        asset_id: AssetId,
        side: Side,
        kind: OrderKind,
        amount: Amount,
        tif: Option<Tif>,
    ) -> Result<Vec<OrderEvent>, OrderBookError> {
        validate_asset(asset_id)?;

        if kind == OrderKind::Market && tif == Some(Tif::GoodUntilCancelled) {
            return Err(OrderBookError::MarketOrderCannotRest);
        }

        let tif = tif.unwrap_or_default();

        // Charge the user's balance for the cost of the order.
        let (collateral_asset_id, collateral_amount) = (collateral_asset(side, asset_id), amount);
        
        if collateral_amount < collateral_asset_id.min_amount() {
            return Err(OrderBookError::OrderAmountTooSmall);
        }

        self.accounts
            .get_mut(&account_id)
            .ok_or(OrderBookError::AccountNotFound)?
            .try_settle(collateral_asset_id, -collateral_amount)
            .map_err(OrderBookError::Account)?;

        let order_id = self.next_order_id;
        self.next_order_id += 1;
        let mut order = Order::new(order_id, account_id, asset_id, side, kind, amount, tif);

        match self.process_order(&mut order) {
            Ok(events) => Ok(events),
            Err(error) => {
                let _ = self.release_for_order(&order);
                Err(error)
            }
        }
    }

    /// Matches `taker` against the opposite side until price limits stop it, then may rest a
    /// remaining GTC limit, refund IOC leftovers, or return `MarketOrderCannotRest`.
    pub fn process_order(&mut self, taker: &mut Order) -> Result<Vec<OrderEvent>, OrderBookError> {
        let mut events = Vec::new();

        while !order_filled_by_remaining(&taker) && let Some(maker_order_id) = match taker.side() {
            Side::Buy => self
                .sells
                .best_order_id_for_asset(&self.orders, taker.asset_id()),
            Side::Sell => self
                .buys
                .best_order_id_for_asset(&self.orders, taker.asset_id()),
        } {
            // Take a snapshot of the maker order to process
            let mut maker = self
                .orders
                .get(&maker_order_id)
                .expect("order book side referenced a missing order")
                .clone();

            let match_result = match self.match_engine.match_orders(&mut maker, taker) {
                Ok(r) => r,
                Err(MatchEngineError::OrderPriceMismatch) => break,
                Err(error) => return Err(OrderBookError::MatchEngine(error)),
            };

            events.push(OrderEvent::Matched {
                taker_order_id: taker.id(),
                maker_order_id,
                match_result: match_result.clone(),
            });

            // Settle traded assets into balances.
            self.apply_match_settlement(&maker, &taker, &match_result)?;

            // Remove the order if filled. Otherwise update into the order book.
            if order_filled_by_remaining(&maker) {
                // Return any dust remaining
                self.release_for_order(&maker);
                self.remove_resting_order(&maker);

                events.push(OrderEvent::Filled {
                    order_id: maker_order_id,
                });
            } else {
                self.orders.insert(maker_order_id, maker);
            }
        }

        // If fully filled, refund dust and push event.
        if order_filled_by_remaining(&taker) {
            // refund any dust reaining
            self.release_for_order(&taker);
            events.push(OrderEvent::Filled {
                order_id: taker.id(),
            });
        } else {
            // Handle TIF if taker order is not fully filled.
            match taker.tif() {
                Tif::GoodUntilCancelled => {
                    self.rest_remaining_taker_order(&taker);
                    events.push(OrderEvent::Added {
                        order_id: taker.id(),
                    });
                }
                Tif::ImmediateOrCancel => {
                    let _ = self.release_for_order(&taker);
                }
            }
        }
        return Ok(events);
    }

    /// Removes a resting limit owned by `account_id` and refunds any remaining collateral for that
    /// order size back to the account balance.
    pub fn cancel_order(
        &mut self,
        account_id: AccountId,
        order_id: OrderId,
    ) -> Result<Vec<OrderEvent>, OrderBookError> {
        let order = self
            .orders
            .get(&order_id)
            .ok_or(OrderBookError::OrderNotFound)?
            .clone();

        if order.account_id() != account_id {
            return Err(OrderBookError::OrderOwnerMismatch);
        }

        self.remove_resting_order(&order);
        self.release_for_order(&order);
        self.accounts.get_mut(&account_id).ok_or(OrderBookError::AccountNotFound)?.unregister_open_order(order_id);

        Ok(vec![OrderEvent::Cancelled {
            order_id: order.id(),
        }])
    }

    pub fn order(&self, order_id: OrderId) -> Option<&Order> {
        self.orders.get(&order_id)
    }

    /// Drops a resting limit from price levels and the id map, and clears its id from the owner's
    /// `Account::open_orders` set.
    fn remove_resting_order(&mut self, order: &Order) {
        let OrderKind::Limit { price } = order.kind() else {
            return;
        };

        match order.side() {
            Side::Buy => self.buys.remove_limit_order(price, order.id()),
            Side::Sell => self.sells.remove_limit_order(price, order.id()),
        }

        self.orders.remove(&order.id());
        if let Some(account) = self.accounts.get_mut(&order.account_id()) {
            account.unregister_open_order(order.id());
        }
    }

    /// After a matched GTC taker with leftover size, re-insert the remainder as a GTC limit at the
    /// same price and track it on the account's open-order set. Returns `true` if a resting row was
    /// written.
    fn rest_remaining_taker_order(&mut self, taker: &Order) {
        let OrderKind::Limit { price } = taker.kind() else {
            return;
        };

        let order_id = taker.id();
        let remaining_order = Order::new(
            order_id,
            taker.account_id(),
            taker.asset_id(),
            taker.side(),
            OrderKind::Limit { price },
            taker.amount(),
            Tif::GoodUntilCancelled,
        );

        match taker.side() {
            Side::Buy => self.buys.add_limit_order(price, order_id),
            Side::Sell => self.sells.add_limit_order(price, order_id),
        }
        self.register_open_order_for_account(taker.account_id(), order_id);
        self.orders.insert(order_id, remaining_order);
    }

    fn register_open_order_for_account(&mut self, account_id: AccountId, order_id: OrderId) {
        if let Some(account) = self.accounts.get_mut(&account_id) {
            account.register_open_order(order_id);
        }
    }

    /// Credits collateral back to the owner's balance.
    fn release_for_order(&mut self, order: &Order) {
        let (asset_id, amount) = collateral_for_order(order);

        if let Some(account) = self.accounts.get_mut(&order.account_id()) {
            account.saturating_settle(asset_id, amount);
        }
    }

    /// Applies `MatchResult` asset flows to buyer and seller accounts (credits only; collateral
    /// was debited when each order was placed).
    fn apply_match_settlement(
        &mut self,
        maker: &Order,
        taker: &Order,
        match_result: &MatchResult,
    ) -> Result<(), OrderBookError> {
        let (buyer, seller) = match (maker.side(), taker.side()) {
            (Side::Buy, Side::Sell) => (maker, taker),
            (Side::Sell, Side::Buy) => (taker, maker),
            _ => return Err(OrderBookError::MatchEngine(MatchEngineError::InvalidOrderSide)),
        };

        if match_result.buyer_base_received > Amount::ZERO {
            let _ = self.accounts
                .get_mut(&buyer.account_id())
                .ok_or(OrderBookError::AccountNotFound)?
                .saturating_settle(buyer.asset_id(), match_result.buyer_base_received);
        }

        if match_result.seller_stable_received > Amount::ZERO {
            let _ = self.accounts
                .get_mut(&seller.account_id())
                .ok_or(OrderBookError::AccountNotFound)?
                .saturating_settle(STABLE_ASSET_ID, match_result.seller_stable_received);
        }

        Ok(())
    }

    pub fn get_account(&self, account_id: AccountId) -> Option<&Account> {
        self.accounts.get(&account_id)
    }
}

/// True when the order's working `amount` is at or below one minimum lot (treat as finished for
/// resting / IOC policy).
fn order_filled_by_remaining(order: &Order) -> bool {
    order.amount() <= min_remaining_for_order(order)
}

/// Smallest positive amount we still treat as economically meaningful for this order's unit
/// (stable for buys, base for sells).
fn min_remaining_for_order(order: &Order) -> Amount {
    match order.side() {
        Side::Buy => STABLE_ASSET_ID.min_amount(),
        Side::Sell => order.asset_id().min_amount(),
    }
}

/// Collateral tied to the current `order.amount` (buy: USDC budget; sell: base size).
fn collateral_for_order(order: &Order) -> (AssetId, Amount) {
    (collateral_asset(order.side(), order.asset_id()), order.amount())
}

fn collateral_asset(side: Side, asset: AssetId) -> AssetId  {
    match side {
        Side::Buy => STABLE_ASSET_ID,
        Side::Sell => asset,
    }
}

pub fn validate_asset(asset_id: AssetId) -> Result<(), OrderBookError> {
    if asset_id == STABLE_ASSET_ID {
        return Err(OrderBookError::InvalidAsset);
    }
    Ok(())
}