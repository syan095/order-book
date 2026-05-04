use order_book::{
    match_engine::{MatchEngine, MatchEngineError, can_match_at_maker_price, limit_price},
    types::{Amount, AssetId, Order, OrderId, OrderKind, Price, STABLE_ASSET_ID, Side, Tif},
};

fn limit_order(id: OrderId, asset_id: AssetId, side: Side, price: Price, amount: Amount) -> Order {
    Order::new(
        id,
        1,
        asset_id,
        side,
        OrderKind::Limit { price },
        amount,
        Tif::GoodUntilCancelled,
    )
}

fn market_order(id: OrderId, asset_id: AssetId, side: Side, amount: Amount) -> Order {
    Order::new(
        id,
        1,
        asset_id,
        side,
        OrderKind::Market,
        amount,
        Tif::GoodUntilCancelled,
    )
}

fn amount(value: i64) -> Amount {
    Amount::from(value)
}

#[test]
fn new_filters_out_stable_asset() {
    let engine = MatchEngine::new([AssetId::Btc, STABLE_ASSET_ID]);

    assert!(engine.supports_asset(AssetId::Btc));
    assert!(!engine.supports_asset(STABLE_ASSET_ID));
}

#[test]
fn add_asset_ignores_stable_asset() {
    let mut engine = MatchEngine::new([]);

    engine.add_asset(STABLE_ASSET_ID);
    engine.add_asset(AssetId::Btc);

    assert!(!engine.supports_asset(STABLE_ASSET_ID));
    assert!(engine.supports_asset(AssetId::Btc));
}

#[test]
fn remove_asset_removes_supported_asset() {
    let mut engine = MatchEngine::new([AssetId::Btc]);

    engine.remove_asset(AssetId::Btc);

    assert!(!engine.supports_asset(AssetId::Btc));
}

#[test]
fn is_supported_asset_requires_same_supported_non_stable_asset() {
    let engine = MatchEngine::new([AssetId::Btc]);
    let maker = limit_order(1, AssetId::Btc, Side::Sell, 100, amount(10));
    let taker = limit_order(2, AssetId::Btc, Side::Buy, 100, amount(500));
    let stable_order = limit_order(3, STABLE_ASSET_ID, Side::Buy, 100, amount(500));

    assert!(engine.is_supported_asset(&maker, &taker));
    assert!(!engine.is_supported_asset(&maker, &stable_order));
}

#[test]
fn limit_price_returns_price_only_for_limit_orders() {
    let limit = limit_order(1, AssetId::Btc, Side::Buy, 100, amount(1_000));
    let market = market_order(2, AssetId::Btc, Side::Buy, amount(1_000));

    assert_eq!(limit_price(&limit), Some(100));
    assert_eq!(limit_price(&market), None);
}

#[test]
fn can_match_at_maker_price_accepts_crossing_limit_taker() {
    let maker_sell = limit_order(1, AssetId::Btc, Side::Sell, 100, amount(10));
    let taker_buy = limit_order(2, AssetId::Btc, Side::Buy, 101, amount(500));
    let maker_buy = limit_order(3, AssetId::Btc, Side::Buy, 100, amount(1_000));
    let taker_sell = limit_order(4, AssetId::Btc, Side::Sell, 99, amount(5));

    assert!(can_match_at_maker_price(&maker_sell, &taker_buy, 100));
    assert!(can_match_at_maker_price(&maker_buy, &taker_sell, 100));
}

#[test]
fn can_match_at_maker_price_rejects_non_crossing_limit_taker() {
    let maker_sell = limit_order(1, AssetId::Btc, Side::Sell, 100, amount(10));
    let taker_buy = limit_order(2, AssetId::Btc, Side::Buy, 99, amount(500));
    let maker_buy = limit_order(3, AssetId::Btc, Side::Buy, 100, amount(1_000));
    let taker_sell = limit_order(4, AssetId::Btc, Side::Sell, 101, amount(5));

    assert!(!can_match_at_maker_price(&maker_sell, &taker_buy, 100));
    assert!(!can_match_at_maker_price(&maker_buy, &taker_sell, 100));
}

#[test]
fn can_match_at_maker_price_accepts_market_taker() {
    let maker_sell = limit_order(1, AssetId::Btc, Side::Sell, 100, amount(10));
    let taker_buy = market_order(2, AssetId::Btc, Side::Buy, amount(500));

    assert!(can_match_at_maker_price(&maker_sell, &taker_buy, 100));
}

#[test]
fn match_orders_matches_limit_buy_taker_against_limit_sell_maker() {
    let engine = MatchEngine::new([AssetId::Btc]);
    let mut maker = limit_order(1, AssetId::Btc, Side::Sell, 100, amount(10));
    let mut taker = limit_order(2, AssetId::Btc, Side::Buy, 105, amount(400));

    let result = engine.match_orders(&mut maker, &mut taker).unwrap();

    assert_eq!(result.amount_traded, amount(4));
    assert_eq!(result.buyer_base_received, amount(4));
    assert_eq!(result.seller_stable_received, amount(400));
    assert!(result.buy_order_filled);
    assert!(!result.sell_order_filled);
    assert_eq!(maker.amount(), amount(6));
    assert_eq!(taker.amount(), amount(0));
}

#[test]
fn match_orders_matches_limit_sell_taker_against_limit_buy_maker() {
    let engine = MatchEngine::new([AssetId::Btc]);
    let mut maker = limit_order(1, AssetId::Btc, Side::Buy, 100, amount(300));
    let mut taker = limit_order(2, AssetId::Btc, Side::Sell, 95, amount(8));

    let result = engine.match_orders(&mut maker, &mut taker).unwrap();

    assert_eq!(result.amount_traded, amount(3));
    assert_eq!(result.buyer_base_received, amount(3));
    assert_eq!(result.seller_stable_received, amount(300));
    assert!(result.buy_order_filled);
    assert!(!result.sell_order_filled);
    assert_eq!(maker.amount(), amount(0));
    assert_eq!(taker.amount(), amount(5));
}

#[test]
fn match_orders_matches_market_buy_taker_at_maker_price() {
    let engine = MatchEngine::new([AssetId::Btc]);
    let mut maker = limit_order(1, AssetId::Btc, Side::Sell, 100, amount(10));
    let mut taker = market_order(2, AssetId::Btc, Side::Buy, amount(600));

    let result = engine.match_orders(&mut maker, &mut taker).unwrap();

    assert_eq!(result.amount_traded, amount(6));
    assert_eq!(result.buyer_base_received, amount(6));
    assert_eq!(result.seller_stable_received, amount(600));
    assert!(result.buy_order_filled);
    assert!(!result.sell_order_filled);
    assert_eq!(maker.amount(), amount(4));
    assert_eq!(taker.amount(), amount(0));
}

#[test]
fn match_orders_matches_market_sell_taker_at_maker_price() {
    let engine = MatchEngine::new([AssetId::Btc]);
    let mut maker = limit_order(1, AssetId::Btc, Side::Buy, 100, amount(1_000));
    let mut taker = market_order(2, AssetId::Btc, Side::Sell, amount(7));

    let result = engine.match_orders(&mut maker, &mut taker).unwrap();

    assert_eq!(result.amount_traded, amount(7));
    assert_eq!(result.buyer_base_received, amount(7));
    assert_eq!(result.seller_stable_received, amount(700));
    assert!(!result.buy_order_filled);
    assert!(result.sell_order_filled);
    assert_eq!(maker.amount(), amount(300));
    assert_eq!(taker.amount(), amount(0));
}

#[test]
fn match_orders_rejects_invalid_asset() {
    let engine = MatchEngine::new([]);
    let mut maker = limit_order(1, AssetId::Btc, Side::Sell, 100, amount(10));
    let mut taker = limit_order(2, AssetId::Btc, Side::Buy, 100, amount(500));

    assert_eq!(
        engine.match_orders(&mut maker, &mut taker),
        Err(MatchEngineError::InvalidAsset)
    );
}

#[test]
fn match_orders_rejects_same_side_orders() {
    let engine = MatchEngine::new([AssetId::Btc]);
    let mut maker = limit_order(1, AssetId::Btc, Side::Buy, 100, amount(1_000));
    let mut taker = limit_order(2, AssetId::Btc, Side::Buy, 100, amount(500));

    assert_eq!(
        engine.match_orders(&mut maker, &mut taker),
        Err(MatchEngineError::InvalidOrderSide)
    );
}

#[test]
fn match_orders_rejects_non_crossing_limit_orders() {
    let engine = MatchEngine::new([AssetId::Btc]);
    let mut maker = limit_order(1, AssetId::Btc, Side::Sell, 100, amount(10));
    let mut taker = limit_order(2, AssetId::Btc, Side::Buy, 99, amount(500));

    assert_eq!(
        engine.match_orders(&mut maker, &mut taker),
        Err(MatchEngineError::OrderPriceMismatch)
    );
}

#[test]
fn match_orders_rejects_market_maker() {
    let engine = MatchEngine::new([AssetId::Btc]);
    let mut maker = market_order(1, AssetId::Btc, Side::Sell, amount(10));
    let mut taker = limit_order(2, AssetId::Btc, Side::Buy, 100, amount(500));

    assert_eq!(
        engine.match_orders(&mut maker, &mut taker),
        Err(MatchEngineError::OrderPriceMismatch)
    );
}
