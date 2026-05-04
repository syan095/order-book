use order_book::types::{Account, AccountError, Amount, AssetId};
use rust_decimal::Decimal;

fn dec(value: i64) -> Decimal {
    Decimal::from(value)
}

#[test]
fn account_new_stores_initial_balances() {
    let account = Account::new(
        1,
        "alice",
        vec![(AssetId::Btc, dec(10)), (AssetId::Usdc, dec(1_000))],
    )
    .unwrap();

    assert_eq!(account.balance(AssetId::Btc), dec(10));
    assert_eq!(account.balance(AssetId::Usdc), dec(1_000));
}

#[test]
fn account_new_rejects_negative_initial_balance() {
    let result = Account::new(1, "alice", vec![(AssetId::Btc, dec(-1))]);

    assert_eq!(result, Err(AccountError::InsufficientBalance));
}

#[test]
fn missing_asset_balance_defaults_to_zero() {
    let account = Account::new(1, "alice", vec![]).unwrap();

    assert_eq!(account.balance(AssetId::Btc), Decimal::ZERO);
}

#[test]
fn set_balance_updates_existing_balance() {
    let mut account = Account::new(1, "alice", vec![(AssetId::Btc, dec(10))]).unwrap();

    account.set_balance(AssetId::Btc, dec(15)).unwrap();

    assert_eq!(account.balance(AssetId::Btc), dec(15));
}

#[test]
fn set_balance_rejects_negative_balance() {
    let mut account = Account::new(1, "alice", vec![(AssetId::Btc, dec(10))]).unwrap();

    let result = account.set_balance(AssetId::Btc, dec(-1));

    assert_eq!(result, Err(AccountError::InsufficientBalance));
    assert_eq!(account.balance(AssetId::Btc), dec(10));
}

#[test]
fn try_settle_can_credit_balance() {
    let mut account = Account::new(1, "alice", vec![(AssetId::Usdc, dec(100))]).unwrap();

    account.try_settle(AssetId::Usdc, Amount::from(25)).unwrap();

    assert_eq!(account.balance(AssetId::Usdc), dec(125));
}

#[test]
fn try_settle_can_debit_balance_when_sufficient() {
    let mut account = Account::new(1, "alice", vec![(AssetId::Usdc, dec(100))]).unwrap();

    account
        .try_settle(AssetId::Usdc, Amount::from(-40))
        .unwrap();

    assert_eq!(account.balance(AssetId::Usdc), dec(60));
}

#[test]
fn try_settle_rejects_debit_that_would_make_balance_negative() {
    let mut account = Account::new(1, "alice", vec![(AssetId::Usdc, dec(100))]).unwrap();

    let result = account.try_settle(AssetId::Usdc, Amount::from(-150));

    assert_eq!(result, Err(AccountError::InsufficientBalance));
    assert_eq!(account.balance(AssetId::Usdc), dec(100));
}

#[test]
fn saturating_settle_returns_zero_remaining_when_fully_settled() {
    let mut account = Account::new(1, "alice", vec![(AssetId::Usdc, dec(100))]).unwrap();

    let remaining = account.saturating_settle(AssetId::Usdc, Amount::from(-40));

    assert_eq!(remaining, Decimal::ZERO);
    assert_eq!(account.balance(AssetId::Usdc), dec(60));
}

#[test]
fn saturating_settle_returns_unsettled_remainder_when_debit_exceeds_balance() {
    let mut account = Account::new(1, "alice", vec![(AssetId::Usdc, dec(100))]).unwrap();

    let remaining = account.saturating_settle(AssetId::Usdc, Amount::from(-150));

    assert_eq!(remaining, dec(-50));
    assert_eq!(account.balance(AssetId::Usdc), Decimal::ZERO);
}

#[test]
fn saturating_settle_credits_full_positive_amount() {
    let mut account = Account::new(1, "alice", vec![(AssetId::Usdc, dec(100))]).unwrap();

    let remaining = account.saturating_settle(AssetId::Usdc, Amount::from(25));

    assert_eq!(remaining, Decimal::ZERO);
    assert_eq!(account.balance(AssetId::Usdc), dec(125));
}
