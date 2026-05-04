//! Integration tests for `OrderBook`: multi-hop matching, market TIF rules, and end-to-end
//! balance / resting-book consistency.

use order_book::{
    order_book::{OrderBook, OrderBookError, OrderEvent},
    types::{Account, Amount, AssetId, OrderKind, OrderId, Side, Tif},
};
use rust_decimal::Decimal;

fn dec(value: i64) -> Decimal {
    Decimal::from(value)
}

fn btc_total_in_system(book: &OrderBook) -> Decimal {
    let mut total = Decimal::ZERO;
    for a in book.accounts().values() {
        total += a.balance(AssetId::Btc);
    }
    for o in book.orders().values() {
        if o.side() == Side::Sell {
            total += o.amount();
        }
    }
    total
}

fn usdc_total_in_system(book: &OrderBook) -> Decimal {
    let mut total = Decimal::ZERO;
    for a in book.accounts().values() {
        total += a.balance(AssetId::Usdc);
    }
    for o in book.orders().values() {
        if o.side() == Side::Buy {
            total += o.amount();
        }
    }
    total
}

fn mk_book_with_accounts<const N: usize>(
    initial_btc: Decimal,
    initial_usdc: Decimal,
) -> OrderBook {
    let mut book = OrderBook::new([AssetId::Btc]);
    for id in 1..=N as u64 {
        let acc = Account::new(
            id,
            format!("trader{id}"),
            vec![(AssetId::Btc, initial_btc), (AssetId::Usdc, initial_usdc)],
        )
        .expect("account");
        book.add_account(acc);
    }
    book
}

#[test]
fn one_aggressive_buy_matches_multiple_resting_sells_fifo_same_price() {
    let mut book = mk_book_with_accounts::<4>(dec(100), dec(1_000_000));

    // Three sells at the same ask price (FIFO in one bucket).
    for account_id in 2..=4 {
        book.place_order(
            account_id,
            AssetId::Btc,
            Side::Sell,
            OrderKind::Limit { price: 100 },
            dec(1),
            None,
        )
        .expect("resting sell");
    }

    let events = book
        .place_order(
            1,
            AssetId::Btc,
            Side::Buy,
            OrderKind::Limit { price: 200 },
            dec(300),
            None,
        )
        .expect("sweeping buy");

    let match_count = events
        .iter()
        .filter(|e| matches!(e, OrderEvent::Matched { .. }))
        .count();
    assert_eq!(match_count, 3, "expected three maker fills");

    let filled_events: Vec<OrderId> = events
        .iter()
        .filter_map(|e| match e {
            OrderEvent::Filled { order_id } => Some(*order_id),
            _ => None,
        })
        .collect();
    assert_eq!(filled_events.len(), 4);
    assert!(filled_events.contains(&1) && filled_events.contains(&2) && filled_events.contains(&3));
    assert!(filled_events.contains(&4));

    assert!(book.order(1).is_none(), "taker buy fully filled, not resting");
    assert!(book.sells.best_price().is_none());

    // Each seller: started 100 BTC, locked 1, earned 100 USDC, unlocked remainder.
    for account_id in 2..=4 {
        let a = book.account(account_id).expect("account");
        assert_eq!(a.balance(AssetId::Btc), dec(99));
        assert_eq!(a.balance(AssetId::Usdc), dec(1_000_000) + dec(100));
        assert!(a.open_orders().is_empty());
    }

    let buyer = book.account(1).expect("buyer");
    assert_eq!(buyer.balance(AssetId::Btc), dec(100) + dec(3));
    assert_eq!(buyer.balance(AssetId::Usdc), dec(1_000_000) - dec(300));
    assert!(buyer.open_orders().is_empty());
}

#[test]
fn market_order_with_good_until_cancelled_is_rejected_at_placement() {
    let mut book = mk_book_with_accounts::<1>(dec(0), dec(1_000));

    let err = book
        .place_order(
            1,
            AssetId::Btc,
            Side::Buy,
            OrderKind::Market,
            dec(100),
            Some(Tif::GoodUntilCancelled),
        )
        .expect_err("market must not use GTC");

    assert_eq!(err, OrderBookError::MarketOrderCannotRest);
}

#[test]
fn ioc_limit_buy_partial_fill_refunds_remaining_quote_and_does_not_rest() {
    let mut book = mk_book_with_accounts::<2>(dec(50), dec(1_000_000));

    book.place_order(
        2,
        AssetId::Btc,
        Side::Sell,
        OrderKind::Limit { price: 100 },
        dec(1),
        None,
    )
    .expect("resting sell");

    let events = book
        .place_order(
            1,
            AssetId::Btc,
            Side::Buy,
            OrderKind::Limit { price: 200 },
            dec(250),
            Some(Tif::ImmediateOrCancel),
        )
        .expect("ioc buy");

    assert!(
        events.iter().any(|e| matches!(e, OrderEvent::Matched { .. })),
        "expected one match"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OrderEvent::Added { .. })),
        "IOC remainder must not rest"
    );

    let buyer = book.account(1).expect("buyer");
    assert_eq!(buyer.balance(AssetId::Btc), dec(50) + dec(1));
    // Debited 250 USDC at placement; 100 spent on fill; 150 refunded (IOC cancel).
    assert_eq!(buyer.balance(AssetId::Usdc), dec(1_000_000) - dec(100));
    assert!(buyer.open_orders().is_empty());
    assert!(book.order(1).is_none(), "maker sell fully filled");
}

#[test]
fn ioc_limit_buy_no_compatible_maker_refunds_full_collateral() {
    let mut book = mk_book_with_accounts::<2>(dec(10), dec(500_000));

    book.place_order(
        2,
        AssetId::Btc,
        Side::Sell,
        OrderKind::Limit { price: 200 },
        dec(5),
        None,
    )
    .expect("resting sell");

    let events = book
        .place_order(
            1,
            AssetId::Btc,
            Side::Buy,
            OrderKind::Limit { price: 150 },
            dec(10_000),
            Some(Tif::ImmediateOrCancel),
        )
        .expect("ioc buy below ask");

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OrderEvent::Matched { .. })),
        "price does not cross the resting ask"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OrderEvent::Added { .. })),
        "IOC must not rest when nothing trades"
    );

    let buyer = book.account(1).expect("buyer");
    assert_eq!(buyer.balance(AssetId::Usdc), dec(500_000));
    assert_eq!(buyer.balance(AssetId::Btc), dec(10));
    assert!(book.sells.best_price() == Some(200));
}

#[test]
fn ioc_limit_buy_full_fill_emits_filled_for_taker() {
    let mut book = mk_book_with_accounts::<2>(dec(50), dec(1_000_000));

    book.place_order(
        2,
        AssetId::Btc,
        Side::Sell,
        OrderKind::Limit { price: 100 },
        dec(1),
        None,
    )
    .expect("resting sell");

    let events = book
        .place_order(
            1,
            AssetId::Btc,
            Side::Buy,
            OrderKind::Limit { price: 200 },
            dec(100),
            Some(Tif::ImmediateOrCancel),
        )
        .expect("ioc buy exact budget");

    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, OrderEvent::Matched { .. }))
            .count(),
        1
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            OrderEvent::Filled { order_id } if *order_id == 2
        )),
        "fully filled IOC taker should get Filled"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OrderEvent::Added { .. })),
        "fully filled IOC should not emit Added"
    );

    let buyer = book.account(1).expect("buyer");
    assert_eq!(buyer.balance(AssetId::Btc), dec(51));
    assert_eq!(buyer.balance(AssetId::Usdc), dec(1_000_000) - dec(100));
}

#[test]
fn ioc_limit_sell_partial_fill_refunds_remaining_base() {
    let mut book = mk_book_with_accounts::<2>(dec(100), dec(1_000_000));

    book.place_order(
        1,
        AssetId::Btc,
        Side::Buy,
        OrderKind::Limit { price: 150 },
        dec(150),
        None,
    )
    .expect("resting buy");

    let events = book
        .place_order(
            2,
            AssetId::Btc,
            Side::Sell,
            OrderKind::Limit { price: 100 },
            dec(10),
            Some(Tif::ImmediateOrCancel),
        )
        .expect("ioc sell");

    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, OrderEvent::Matched { .. }))
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OrderEvent::Added { .. })),
        "IOC sell remainder must not rest"
    );

    let seller = book.account(2).expect("seller");
    // Started 100 BTC; 10 locked for order; 1 sold at maker 150 -> 9 left refunded + 90 USDC.
    assert_eq!(seller.balance(AssetId::Btc), dec(99));
    assert_eq!(seller.balance(AssetId::Usdc), dec(1_000_000) + dec(150));

    let maker = book.order(1);
    assert!(
        maker.is_none(),
        "maker buy fully filled against first slice"
    );
}

#[test]
fn simulated_ten_orders_same_asset_conserves_balances_and_resting_book() {
    const N: usize = 10;
    let initial_btc = dec(500);
    let initial_usdc = dec(10_000_000);
    let mut book = mk_book_with_accounts::<N>(initial_btc, initial_usdc);

    let btc_before = btc_total_in_system(&book);
    let usdc_before = usdc_total_in_system(&book);

    let mut all_events: Vec<OrderEvent> = Vec::new();

    // Ten deliberately interleaved limits on BTC/USDC. Maker price is always the resting limit.
    // O1: deep bid (rests)
    all_events.extend(
        book.place_order(
            1,
            AssetId::Btc,
            Side::Buy,
            OrderKind::Limit { price: 50 },
            dec(50_000),
            None,
        )
        .unwrap(),
    );
    // O2: tight ask above O1 — no cross (rests)
    all_events.extend(
        book.place_order(
            2,
            AssetId::Btc,
            Side::Sell,
            OrderKind::Limit { price: 60 },
            dec(40),
            None,
        )
        .unwrap(),
    );
    // O3: aggressive buy — lifts O2 at 60 until O2's size or budget exhausted
    all_events.extend(
        book.place_order(
            3,
            AssetId::Btc,
            Side::Buy,
            OrderKind::Limit { price: 70 },
            dec(1_200),
            None,
        )
        .unwrap(),
    );
    // O4: another ask inside the spread (rests after any partial logic)
    all_events.extend(
        book.place_order(
            4,
            AssetId::Btc,
            Side::Sell,
            OrderKind::Limit { price: 58 },
            dec(10),
            None,
        )
        .unwrap(),
    );
    // O5: sell hits best bid chain (highest bid first: O3 remainder, then O1)
    all_events.extend(
        book.place_order(
            5,
            AssetId::Btc,
            Side::Sell,
            OrderKind::Limit { price: 55 },
            dec(25),
            None,
        )
        .unwrap(),
    );
    // O6: large bid to clear cheap asks (O4 @58) and walk up
    all_events.extend(
        book.place_order(
            6,
            AssetId::Btc,
            Side::Buy,
            OrderKind::Limit { price: 65 },
            dec(5_000),
            None,
        )
        .unwrap(),
    );
    // O7: market IOC buy — consumes remaining visible asks
    all_events.extend(
        book.place_order(
            7,
            AssetId::Btc,
            Side::Buy,
            OrderKind::Market,
            dec(800),
            Some(Tif::ImmediateOrCancel),
        )
        .unwrap(),
    );
    // O8: sell into bids
    all_events.extend(
        book.place_order(
            8,
            AssetId::Btc,
            Side::Sell,
            OrderKind::Limit { price: 52 },
            dec(8),
            None,
        )
        .unwrap(),
    );
    // O9: resting bid
    all_events.extend(
        book.place_order(
            9,
            AssetId::Btc,
            Side::Buy,
            OrderKind::Limit { price: 48 },
            dec(10_000),
            None,
        )
        .unwrap(),
    );
    // O10: IOC sell — take what bids allow, cancel remainder
    all_events.extend(
        book.place_order(
            10,
            AssetId::Btc,
            Side::Sell,
            OrderKind::Limit { price: 49 },
            dec(500),
            Some(Tif::ImmediateOrCancel),
        )
        .unwrap(),
    );

    let match_count = all_events
        .iter()
        .filter(|e| matches!(e, OrderEvent::Matched { .. }))
        .count();
    assert!(
        match_count >= 6,
        "expected several multi-hop matches across 10 orders, got {match_count}"
    );

    for e in &all_events {
        if let OrderEvent::Matched { match_result, .. } = e {
            assert!(match_result.amount_traded > Amount::ZERO);
            assert_eq!(
                match_result.buyer_base_received,
                match_result.amount_traded
            );
            assert!(match_result.seller_stable_received > Amount::ZERO);
        }
    }

    assert_eq!(
        btc_total_in_system(&book),
        btc_before,
        "BTC must be conserved (balances + locked sell size)"
    );
    assert_eq!(
        usdc_total_in_system(&book),
        usdc_before,
        "USDC must be conserved (balances + locked buy budgets)"
    );

    // Every match event should reference valid maker/taker ids and positive trade sizes.
    // (Re-run placement is not available; we only assert global invariants + book structure.)

    // Bids strictly below asks when both sides have liquidity (spread positive).
    let best_bid = book.buys.best_price();
    let best_ask = book.sells.best_price();
    if let (Some(bid), Some(ask)) = (best_bid, best_ask) {
        assert!(bid < ask, "crossed book: bid {bid} >= ask {ask}");
    }

    // Open-order tracking matches resting ids for each account.
    for account_id in 1..=N as u64 {
        let acc = book.account(account_id).expect("account");
        for &oid in acc.open_orders() {
            assert!(
                book.order(oid).is_some(),
                "account {account_id} lists open {oid} but order missing"
            );
        }
    }

    // No zero-amount ghosts
    for (&id, o) in book.orders() {
        assert!(
            o.amount() > Amount::ZERO,
            "order {id} should not rest with zero working amount"
        );
    }
}
