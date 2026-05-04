use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Display,
    string::FromUtf8Error,
};

use rust_decimal::Decimal;

pub const STABLE_ASSET_ID: AssetId = AssetId::Usdc;
pub const USDC_DECIMALS: u8 = 6;
pub const BTC_DECIMALS: u8 = 8;

pub type OrderId = u64;
pub type Price = u64;
pub type Quantity = u64;
pub type AccountId = u64;
pub type Amount = Decimal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderKind {
    Limit { price: Price },
    Market,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderBookError {
    MarketOrderCannotRest,
    OrderWouldMatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    id: OrderId,
    account_id: AccountId,
    asset_id: AssetId,
    side: Side,
    kind: OrderKind,
    amount: Amount,
    tif: Tif,
}

impl Order {
    pub fn new(
        id: OrderId,
        account_id: AccountId,
        asset_id: AssetId,
        side: Side,
        kind: OrderKind,
        amount: Amount,
        tif: Tif,
    ) -> Self {
        Self {
            id,
            account_id,
            asset_id,
            side,
            kind,
            amount,
            tif,
        }
    }

    pub fn id(&self) -> OrderId {
        self.id
    }

    pub fn account_id(&self) -> AccountId {
        self.account_id
    }

    pub fn asset_id(&self) -> AssetId {
        self.asset_id
    }

    pub fn side(&self) -> Side {
        self.side
    }

    pub fn kind(&self) -> OrderKind {
        self.kind
    }

    pub fn amount(&self) -> Amount {
        self.amount
    }

    /// Decreases working size (buy: USDC budget; sell: base quantity), floored at zero.
    pub fn reduce_amount_saturating(&mut self, by: Amount) {
        if by <= Amount::ZERO {
            return;
        }
        self.amount = (self.amount - by).max(Amount::ZERO);
    }

    pub fn tif(&self) -> Tif {
        self.tif
    }
}

#[derive(Debug, Default)]
pub struct OrderBucket {
    order_ids: Vec<OrderId>,
}

impl OrderBucket {
    fn push(&mut self, order_id: OrderId) {
        self.order_ids.push(order_id);
    }

    fn first_order_id_for_asset(
        &self,
        orders: &HashMap<OrderId, Order>,
        asset_id: AssetId,
    ) -> Option<OrderId> {
        self.order_ids.iter().copied().find(|order_id| {
            orders
                .get(order_id)
                .is_some_and(|order| order.asset_id() == asset_id)
        })
    }

    fn remove(&mut self, order_id: OrderId) {
        self.order_ids
            .retain(|existing_id| *existing_id != order_id);
    }

    fn is_empty(&self) -> bool {
        self.order_ids.is_empty()
    }
}

#[derive(Debug)]
pub struct OrderBookSide {
    side: Side,
    price_levels: BTreeMap<Price, OrderBucket>,
}

impl OrderBookSide {
    pub(crate) fn new(side: Side) -> Self {
        Self {
            side,
            price_levels: BTreeMap::new(),
        }
    }

    pub(crate) fn add_limit_order(&mut self, price: Price, order_id: OrderId) {
        self.price_levels.entry(price).or_default().push(order_id);
    }

    pub(crate) fn remove_limit_order(&mut self, price: Price, order_id: OrderId) {
        if let Some(bucket) = self.price_levels.get_mut(&price) {
            bucket.remove(order_id);

            if bucket.is_empty() {
                self.price_levels.remove(&price);
            }
        }
    }

    pub(crate) fn best_order_id_for_asset(
        &self,
        orders: &HashMap<OrderId, Order>,
        asset_id: AssetId,
    ) -> Option<OrderId> {
        match self.side {
            Side::Buy => self
                .price_levels
                .iter()
                .rev()
                .find_map(|(_, bucket)| bucket.first_order_id_for_asset(orders, asset_id)),
            Side::Sell => self
                .price_levels
                .iter()
                .find_map(|(_, bucket)| bucket.first_order_id_for_asset(orders, asset_id)),
        }
    }

    pub fn best_price(&self) -> Option<Price> {
        match self.side {
            Side::Buy => self.price_levels.keys().next_back().copied(),
            Side::Sell => self.price_levels.keys().next().copied(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetId {
    Usdc,
    Btc,
}

impl AssetId {
    pub fn as_str(&self) -> &str {
        match self {
            AssetId::Usdc => "usdc",
            AssetId::Btc => "btc",
        }
    }

    pub fn decimals(&self) -> u8 {
        match self {
            AssetId::Usdc => USDC_DECIMALS,
            AssetId::Btc => BTC_DECIMALS,
        }
    }

    pub fn min_amount(&self) -> Amount {
        Amount::new(1, self.decimals() as u32)
    }
}

impl Into<u64> for AssetId {
    fn into(self) -> u64 {
        match self {
            AssetId::Usdc => 0,
            AssetId::Btc => 1,
        }
    }
}

pub struct Asset {
    id: AssetId,
    name: Vanity,
}

impl Asset {
    pub fn new(id: AssetId, name: impl Into<Vanity>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }

    pub fn id(&self) -> AssetId {
        self.id
    }

    pub fn name(&self) -> &Vanity {
        &self.name
    }

    pub fn decimals(&self) -> u8 {
        self.id.decimals()
    }

    pub fn min_amount(&self) -> Amount {
        Amount::new(1, self.decimals() as u32)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountError {
    InsufficientBalance,
    AmountTooSmall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vanity(Vec<u8>);

impl Vanity {
    pub fn new(value: impl Into<Vec<u8>>) -> Self {
        Self(value.into())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    pub fn into_string(self) -> Result<String, FromUtf8Error> {
        String::from_utf8(self.0)
    }
}

impl TryFrom<Vanity> for String {
    type Error = FromUtf8Error;

    fn try_from(value: Vanity) -> Result<Self, Self::Error> {
        value.into_string()
    }
}

impl From<String> for Vanity {
    fn from(value: String) -> Self {
        Self(value.into_bytes())
    }
}

impl From<&str> for Vanity {
    fn from(value: &str) -> Self {
        Self(value.as_bytes().to_vec())
    }
}

impl From<Vec<u8>> for Vanity {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

impl Display for Vanity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(&self.0))
    }
}

/// User wallet in this toy exchange: per-asset balances and ids of resting orders on the book.
/// Collateral for an order is held by debiting `balance` at placement time (no separate escrow).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    id: AccountId,
    name: Vanity,
    balance: HashMap<AssetId, Decimal>,
    /// Resting limit order ids belonging to this account (kept in sync by `OrderBook`).
    orders: HashSet<OrderId>,
}

impl Account {
    pub fn new(
        id: AccountId,
        name: impl Into<Vanity>,
        balance: Vec<(AssetId, Decimal)>,
    ) -> Result<Self, AccountError> {
        if balance.iter().any(|(_, balance)| *balance < Decimal::ZERO) {
            return Err(AccountError::InsufficientBalance);
        }

        let balance = balance.into_iter().collect();
        Ok(Self {
            id,
            name: name.into(),
            balance,
            orders: HashSet::new(),
        })
    }

    pub fn id(&self) -> AccountId {
        self.id
    }

    pub fn balance(&self, asset_id: AssetId) -> Decimal {
        self.balance
            .get(&asset_id)
            .copied()
            .unwrap_or(Decimal::ZERO)
    }

    pub fn open_orders(&self) -> &HashSet<OrderId> {
        &self.orders
    }

    pub fn register_open_order(&mut self, order_id: OrderId) {
        self.orders.insert(order_id);
    }

    pub fn unregister_open_order(&mut self, order_id: OrderId) {
        self.orders.remove(&order_id);
    }

    pub fn set_balance(&mut self, asset_id: AssetId, balance: Decimal) -> Result<(), AccountError> {
        if balance < Decimal::ZERO {
            return Err(AccountError::InsufficientBalance);
        }

        self.balance.insert(asset_id, balance);
        Ok(())
    }

    /// Adjusts `balance(asset_id)` by `amount` (negative debits, positive credits); rejects if the
    /// balance would go negative.
    pub fn try_settle(&mut self, asset_id: AssetId, amount: Amount) -> Result<(), AccountError> {
        let result_balance = self.balance(asset_id) + amount;

        if result_balance < Decimal::ZERO {
            return Err(AccountError::InsufficientBalance);
        }

        self.set_balance(asset_id, result_balance)
    }

    pub fn saturating_settle(&mut self, asset_id: AssetId, amount: Amount) -> Amount {
        let current_balance = self.balance(asset_id);
        let settled_amount = if current_balance + amount < Decimal::ZERO {
            -current_balance
        } else {
            amount
        };

        self.balance
            .insert(asset_id, current_balance + settled_amount);
        amount - settled_amount
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tif {
    #[default]
    GoodUntilCancelled,
    ImmediateOrCancel,
}

/// For displaying account information to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountInfo {
    id: AccountId,
    name: Vanity,
    total_balances: HashMap<AssetId, Decimal>,
    free_balance: HashMap<AssetId, Decimal>,
    orders: HashSet<OrderId>,
}

impl AccountInfo {
    pub fn new(account: &Account, orders: HashSet<&Order>) -> Self {
        let mut total_balances = account.balance.clone();
        orders.iter().for_each(|order| {
            total_balances.entry(order.asset_id()).and_modify(|balance| *balance += order.amount()).or_insert(order.amount());
        });
        
        Self {
            id: account.id(),
            name: account.name.clone(),
            total_balances,
            free_balance: account.balance.clone(),
            orders: account.orders.clone(),
        }
    }
}

impl Display for AccountInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Account {}: id={} \ntotal_balances: {:?} \nfree_balance: {:?} \norders: {:?} }}", self.id, self.name, self.total_balances, self.free_balance, self.orders)
    }
}