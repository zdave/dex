use crate::{mock::*, Error};
use frame_support::{assert_noop, assert_ok};
use sp_runtime::{ArithmeticError, DispatchResult};

fn create_assets() -> DispatchResult {
    Assets::force_create(Origin::root(), 0, 1, true, 10)?;
    Assets::force_create(Origin::root(), 1, 1, true, 20)?;
    Assets::force_create(Origin::root(), 2, 1, true, 30)?;

    Assets::mint(Origin::signed(1), 0, 1, 10_000)?;
    Assets::mint(Origin::signed(1), 1, 1, 10_000)?;
    Assets::mint(Origin::signed(1), 2, 1, 10_000)?;

    Assets::mint(Origin::signed(1), 0, 2, 10_000)?;
    Assets::mint(Origin::signed(1), 1, 2, 10_000)?;
    Assets::mint(Origin::signed(1), 2, 2, 10_000)?;

    Ok(())
}

#[test]
fn basic_add_remove_liquidity() {
    new_test_ext().execute_with(|| {
        assert_ok!(create_assets());
        assert_ok!(Cfmm::add_liquidity(Origin::signed(1), 0, 0, 1_000, 1, 0, 2_000));
        assert_eq!(Cfmm::get_exchange_rate(0, 1), (1_000, 2_000));
        assert_ok!(Cfmm::add_liquidity(Origin::signed(2), 0, 0, 500, 1, 0, 1_000));
        assert_eq!(Cfmm::get_exchange_rate(1, 0), (3_000, 1_500));
        assert_ok!(Cfmm::remove_liquidity(Origin::signed(1), 0, 1, 20_000));
        assert_eq!(Cfmm::get_exchange_rate(0, 1), (500, 1_000));
        assert_ok!(Cfmm::remove_liquidity(Origin::signed(2), 0, 1, 10_000));
        assert_eq!(Cfmm::get_exchange_rate(0, 1), (0, 0));
    });
}

#[test]
fn add_liquidity_insufficient_assets() {
    new_test_ext().execute_with(|| {
        assert_ok!(create_assets());
        assert_noop!(
            Cfmm::add_liquidity(Origin::signed(1), 0, 0, 15_000, 1, 0, 2_000),
            pallet_assets::pallet::Error::<Test>::BalanceLow
        );
        assert_noop!(
            Cfmm::add_liquidity(Origin::signed(1), 0, 0, 1_000, 1, 0, 25_000),
            pallet_assets::pallet::Error::<Test>::BalanceLow
        );
    });
}

#[test]
fn add_liquidity_maintain_exchange_rate() {
    new_test_ext().execute_with(|| {
        assert_ok!(create_assets());
        assert_ok!(Cfmm::add_liquidity(Origin::signed(1), 0, 0, 1_000, 1, 0, 2_000));
        assert_eq!(Cfmm::get_exchange_rate(0, 1), (1_000, 2_000));
        assert_noop!(
            Cfmm::add_liquidity(Origin::signed(1), 0, 1_001, 2_000, 1, 0, 2_000),
            Error::<Test>::UnexpectedExchangeRate
        );
        assert_ok!(Cfmm::add_liquidity(Origin::signed(1), 0, 0, 2_000, 1, 0, 2_000));
        assert_ok!(Cfmm::add_liquidity(Origin::signed(2), 0, 0, 2_000, 1, 0, 2_000));
        assert_eq!(Cfmm::get_exchange_rate(0, 1), (3_000, 6_000));
    });
}

#[test]
fn add_liquidity_one_asset() {
    new_test_ext().execute_with(|| {
        assert_ok!(create_assets());
        assert_noop!(
            Cfmm::add_liquidity(Origin::signed(1), 0, 0, 1_000, 0, 0, 1_000),
            Error::<Test>::AssetsIdentical
        );
    });
}

#[test]
fn add_too_little_liquidity() {
    new_test_ext().execute_with(|| {
        assert_ok!(create_assets());
        assert_noop!(
            Cfmm::add_liquidity(Origin::signed(1), 0, 0, 99, 1, 0, 200),
            Error::<Test>::InsufficientPoolAmount
        );
        assert_noop!(
            Cfmm::add_liquidity(Origin::signed(1), 0, 0, 100, 1, 0, 199),
            Error::<Test>::InsufficientPoolAmount
        );
        assert_ok!(Cfmm::add_liquidity(Origin::signed(1), 0, 0, 100, 1, 0, 200));
        assert_eq!(Cfmm::get_exchange_rate(0, 1), (100, 200));
    });
}

#[test]
fn remove_too_much_liquidity() {
    new_test_ext().execute_with(|| {
        assert_ok!(create_assets());
        assert_ok!(Cfmm::add_liquidity(Origin::signed(1), 0, 0, 1_000, 1, 0, 2_000));
        assert_eq!(Cfmm::get_exchange_rate(0, 1), (1_000, 2_000));
        assert_noop!(
            Cfmm::remove_liquidity(Origin::signed(1), 0, 1, 20_001),
            ArithmeticError::Underflow
        );
        assert_noop!(
            Cfmm::remove_liquidity(Origin::signed(1), 0, 1, 18_100),
            Error::<Test>::InsufficientPoolAmount
        );
        assert_ok!(Cfmm::remove_liquidity(Origin::signed(1), 0, 1, 18_000));
        assert_eq!(Cfmm::get_exchange_rate(0, 1), (100, 200));
        assert_ok!(Cfmm::remove_liquidity(Origin::signed(1), 0, 1, 2_000));
        assert_eq!(Cfmm::get_exchange_rate(0, 1), (0, 0));
    });
}

#[test]
fn below_min_balance_transferred_not_burned() {
    new_test_ext().execute_with(|| {
        assert_ok!(create_assets());
        assert_ok!(Cfmm::add_liquidity(Origin::signed(1), 0, 0, 9_990, 1, 0, 9_980));
        assert_eq!(Cfmm::get_exchange_rate(0, 1), (9_990, 9_980));
        assert_ok!(Cfmm::remove_liquidity(Origin::signed(1), 0, 1, 99_900));
        assert_ok!(Cfmm::add_liquidity(Origin::signed(1), 0, 0, 9_991, 1, 0, 9_981));
        assert_eq!(Cfmm::get_exchange_rate(0, 1), (10_000, 10_000));
        assert_ok!(Cfmm::remove_liquidity(Origin::signed(1), 0, 1, 99_910));
        assert_eq!(Cfmm::get_exchange_rate(0, 1), (0, 0));
    });
}

#[test]
fn exchange_no_liquidity() {
    new_test_ext().execute_with(|| {
        assert_ok!(create_assets());
        assert_noop!(Cfmm::exchange(Origin::signed(1), 0, 1_000, 1, 0), Error::<Test>::NoLiquidity);
    });
}

#[test]
fn basic_exchange() {
    new_test_ext().execute_with(|| {
        assert_ok!(create_assets());
        assert_ok!(Cfmm::add_liquidity(Origin::signed(1), 0, 0, 5_000, 1, 0, 10_000));
        assert_noop!(
            Cfmm::exchange(Origin::signed(2), 0, 20, 1, 36),
            Error::<Test>::UnexpectedExchangeRate
        );
        assert_ok!(Cfmm::exchange(Origin::signed(2), 0, 20, 1, 35));
        assert_eq!(Assets::balance(0, 2), 9_980);
        assert_eq!(Assets::balance(1, 2), 10_035);
        assert_eq!(Cfmm::get_exchange_rate(0, 1), (5_020, 9_965));
    });
}
