use std::collections::HashSet;

use crate::types::{Amount, AssetId, Order, OrderId, OrderKind, Price, STABLE_ASSET_ID, Side};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchResult {
    pub amount_traded: Amount,
    /// Base size credited to the buyer's account for this hop.
    pub buyer_base_received: Amount,
    /// Quote (stable) credited to the seller's account for this hop.
    pub seller_stable_received: Amount,
    pub buy_order_filled: bool,
    pub sell_order_filled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchEngine {
    // Non-stable assets this engine can trade against STABLE_ASSET_ID.
    supported_asset: HashSet<AssetId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchEngineError {
    InvalidAsset,
    OrderPriceMismatch,
    InvalidOrderSide,
}

impl MatchEngine {
    pub fn new(supported_asset: impl IntoIterator<Item = AssetId>) -> Self {
        Self {
            supported_asset: supported_asset
                .into_iter()
                .filter(|asset| *asset != STABLE_ASSET_ID)
                .collect(),
        }
    }

    pub fn add_asset(&mut self, asset: AssetId) {
        if asset != STABLE_ASSET_ID {
            self.supported_asset.insert(asset);
        }
    }

    pub fn remove_asset(&mut self, asset: AssetId) {
        self.supported_asset.remove(&asset);
    }

    pub fn supports_asset(&self, asset: AssetId) -> bool {
        self.supported_asset.contains(&asset)
    }

    /// Prices and sizes the trade, mutates both orders' working `amount` (buy: −stable spent,
    /// sell: −base traded), and returns what to credit to each party's balance (positive legs only).
    pub fn match_orders(
        &self,
        maker: &mut Order,
        taker: &mut Order,
    ) -> Result<MatchResult, MatchEngineError> {
        if !self.is_supported_asset(maker, taker) {
            return Err(MatchEngineError::InvalidAsset);
        }

        if maker.side() == taker.side() {
            return Err(MatchEngineError::InvalidOrderSide);
        }

        let maker_price = limit_price(maker).ok_or(MatchEngineError::OrderPriceMismatch)?;

        if !can_match_at_maker_price(maker, taker, maker_price) {
            return Err(MatchEngineError::OrderPriceMismatch);
        }

        let (buy_order, sell_order) = match (maker.side(), taker.side()) {
            (Side::Buy, Side::Sell) => (maker, taker),
            (Side::Sell, Side::Buy) => (taker, maker),
            _ => return Err(MatchEngineError::InvalidOrderSide),
        };

        let traded_asset_amount =
            buy_capacity_at_price(buy_order, maker_price).min(sell_order.amount());
        let stable_asset_amount = traded_asset_amount * Amount::from(maker_price);

        buy_order.reduce_amount_saturating(stable_asset_amount);
        sell_order.reduce_amount_saturating(traded_asset_amount);

        let min_stable = STABLE_ASSET_ID.min_amount();
        let min_traded_asset = sell_order.asset_id().min_amount();

        Ok(MatchResult {
            amount_traded: traded_asset_amount,
            buyer_base_received: traded_asset_amount,
            seller_stable_received: stable_asset_amount,
            buy_order_filled: buy_order.amount() <= min_stable,
            sell_order_filled: sell_order.amount() <= min_traded_asset,
        })
    }

    pub fn is_supported_asset(&self, maker: &Order, taker: &Order) -> bool {
        maker.asset_id() == taker.asset_id()
            && maker.asset_id() != STABLE_ASSET_ID
            && self.supported_asset.contains(&maker.asset_id())
    }
}

pub fn can_match_at_maker_price(maker: &Order, taker: &Order, maker_price: Price) -> bool {
    match (maker.side(), taker.kind()) {
        (Side::Sell, OrderKind::Limit { price }) => price >= maker_price,
        (Side::Buy, OrderKind::Limit { price }) => price <= maker_price,
        (_, OrderKind::Market) => true,
    }
}

pub fn limit_price(order: &Order) -> Option<Price> {
    match order.kind() {
        OrderKind::Limit { price } => Some(price),
        OrderKind::Market => None,
    }
}

fn buy_capacity_at_price(order: &Order, price: Price) -> Amount {
    order.amount() / Amount::from(price)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchEngineEvent {
    OrderMatched(OrderId, OrderId, MatchResult),
}
