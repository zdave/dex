#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

#[frame_support::pallet]
pub mod pallet {
    use frame_support::{
        pallet_prelude::*,
        traits::{
            fungibles::{Inspect, Transfer},
            tokens,
        },
        transactional, PalletId,
    };
    use frame_system::pallet_prelude::*;
    use sp_core::U256;
    use sp_runtime::{
        traits::{AccountIdConversion, CheckedAdd, CheckedMul, CheckedSub, Saturating, Zero},
        ArithmeticError, Permill,
    };
    use sp_std::cmp::{max, min};

    /// Type for result of multiplying two `AssetBalance`s together. Just fixed as `U256` for now.
    /// Could probably be smarter and use something like `overflow_prune_mul` from `per_things` to
    /// avoid needing large intermediate results.
    type BalanceMulResult = U256;

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// Because this pallet emits events, it depends on the runtime's definition of an event.
        type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

        /// Used to derive `AccountId`s for the liquidity pools.
        #[pallet::constant]
        type PalletId: Get<PalletId>;

        type AssetId: tokens::AssetId + MaxEncodedLen;
        type AssetBalance: tokens::Balance
            + MaxEncodedLen
            + Into<BalanceMulResult>
            + TryFrom<BalanceMulResult>;
        type Fungibles: Transfer<
            Self::AccountId,
            AssetId = Self::AssetId,
            Balance = Self::AssetBalance,
        >;

        /// When adding or removing liquidity, we require that the final amount of each asset in
        /// the liquidity pool effectively owned by the sender be at least a certain multiple of
        /// the minimum balance. The purpose of this is to prevent griefing when the liquidity pool
        /// is very small: if someone intentionally adds liquidity to the pool with a bogus
        /// exchange rate (always possible when the liquidity pool is empty), it should be possible
        /// to _profitably_ correct the exchange rate via arbitrage.
        ///
        /// Note that it is possible for the amount of an asset in a liquidity pool effectively
        /// owned by a liquidity provider to fall below this minimum as exchanges happen.
        #[pallet::constant]
        type PoolMinAmountMultiple: Get<Self::AssetBalance>;

        /// The amount of liquidity tokens given to the first liquidity provider for an asset pair
        /// is determined by the largest of either asset amount multiplied by this. This number is
        /// somewhat arbitrary, but determines how accurately the liquidity pool can be divided up
        /// amongst liquidity providers.
        ///
        /// Note that the overall amount of assets in the liquidity pool will rise over time due to
        /// fees, whereas the amount of liquidity tokens will not (unless new liquidity is added).
        /// Also, the balance of assets in the pool may change as exchanges are performed.
        #[pallet::constant]
        type InitialLiquidityPerAssetUnit: Get<LiquidityBalanceOf<Self>>;

        /// This portion of the source amount for each exchange will be added to the pool as a fee;
        /// the remainder will be exchanged.
        #[pallet::constant]
        type ExchangeFee: Get<Permill>;
    }

    type AssetIdOf<T> =
        <<T as Config>::Fungibles as Inspect<<T as frame_system::Config>::AccountId>>::AssetId;
    type AssetIdPairOf<T> = (AssetIdOf<T>, AssetIdOf<T>);
    type AssetBalanceOf<T> =
        <<T as Config>::Fungibles as Inspect<<T as frame_system::Config>::AccountId>>::Balance;
    type LiquidityBalanceOf<T> = AssetBalanceOf<T>;

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    /// Track the total liquidity of each asset pair. Note that this means the number of liquidity
    /// tokens that have been handed out to liquidity providers, not the count of assets in the
    /// pool.
    #[pallet::storage]
    pub type TotalLiquidity<T> =
        StorageMap<_, Blake2_128Concat, AssetIdPairOf<T>, LiquidityBalanceOf<T>, ValueQuery>;

    /// Track the liquidity provided for each asset pair by each account.
    ///
    /// Guess that it's probably more useful to be able to efficiently iterate over all liquidity
    /// provided by an account than all liquidity provided for an asset pair; total liquidity for
    /// an asset pair is already easily available via `TotalLiquidity`.
    #[pallet::storage]
    pub type Liquidity<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        Blake2_128Concat,
        AssetIdPairOf<T>,
        LiquidityBalanceOf<T>,
        ValueQuery,
    >;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        LiquidityAdded {
            who: T::AccountId,
            asset_a: AssetIdOf<T>,
            amount_a: AssetBalanceOf<T>,
            asset_b: AssetIdOf<T>,
            amount_b: AssetBalanceOf<T>,
            liquidity: LiquidityBalanceOf<T>,
        },
        LiquidityRemoved {
            who: T::AccountId,
            asset_a: AssetIdOf<T>,
            amount_a: AssetBalanceOf<T>,
            asset_b: AssetIdOf<T>,
            amount_b: AssetBalanceOf<T>,
            liquidity: LiquidityBalanceOf<T>,
        },
        Exchanged {
            who: T::AccountId,
            source_asset: AssetIdOf<T>,
            source_amount: AssetBalanceOf<T>,
            dest_asset: AssetIdOf<T>,
            dest_amount: AssetBalanceOf<T>,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// The two assets provided were identical; it does not make sense to exchange between
        /// them.
        AssetsIdentical,
        /// The sender did not leave enough of each asset in the liquidity pool for the asset pair.
        InsufficientPoolAmount,
        /// The liquidity pool for the asset pair is empty.
        NoLiquidity,
        /// The transaction was aborted as the effective exchange rate was too far from that
        /// expected by the sender.
        UnexpectedExchangeRate,
    }

    fn make_asset_pair<T: Config>(
        a: AssetIdOf<T>,
        b: AssetIdOf<T>,
    ) -> Result<AssetIdPairOf<T>, DispatchError> {
        ensure!(a != b, Error::<T>::AssetsIdentical);
        Ok(if a.encode() < b.encode() { (a, b) } else { (b, a) })
    }

    fn get_pool_account<T: Config>(asset_pair: AssetIdPairOf<T>) -> T::AccountId {
        T::PalletId::get().into_sub_account_truncating(asset_pair)
    }

    fn add<T: CheckedAdd>(a: T, b: T) -> Result<T, ArithmeticError> {
        a.checked_add(&b).ok_or(ArithmeticError::Overflow)
    }

    fn sub<T: CheckedSub>(a: T, b: T) -> Result<T, ArithmeticError> {
        a.checked_sub(&b).ok_or(ArithmeticError::Underflow)
    }

    fn mul<T: Into<BalanceMulResult>>(a: T, b: T) -> Result<BalanceMulResult, ArithmeticError> {
        <T as Into<BalanceMulResult>>::into(a)
            .checked_mul(b.into())
            .ok_or(ArithmeticError::Overflow)
    }

    /// `floor((a * b) / c)`
    fn mul_div_floor<T: Into<BalanceMulResult> + TryFrom<BalanceMulResult>>(
        a: T,
        b: T,
        c: T,
    ) -> Result<T, ArithmeticError> {
        let res: BalanceMulResult =
            mul(a, b)?.checked_div(c.into()).ok_or(ArithmeticError::DivisionByZero)?;
        <T as TryFrom<BalanceMulResult>>::try_from(res).map_err(|_| ArithmeticError::Overflow)
    }

    /// `ceil((a * b) / c)`
    fn mul_div_ceil<T: Into<BalanceMulResult> + TryFrom<BalanceMulResult> + Copy>(
        a: T,
        b: T,
        c: T,
    ) -> Result<T, ArithmeticError> {
        let c_minus_one = <T as Into<BalanceMulResult>>::into(c)
            .checked_sub(1u32.into())
            .ok_or(ArithmeticError::Underflow)?;
        let biased = mul(a, b)?.checked_add(c_minus_one).ok_or(ArithmeticError::Overflow)?;
        let res: BalanceMulResult =
            biased.checked_div(c.into()).ok_or(ArithmeticError::DivisionByZero)?;
        <T as TryFrom<BalanceMulResult>>::try_from(res).map_err(|_| ArithmeticError::Overflow)
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Add liquidity for an asset pair.
        ///
        /// If the sender is the first liquidity provider for the given asset pair, the full
        /// amounts will be added to the liquidity pool. Otherwise, an equivalent value of each
        /// asset will be transferred, according to the current exchange rate.
        ///
        /// If the actual amount of either asset added would be less than the provided minimum, the
        /// transaction is aborted. The purpose of this is to protect the sender against
        /// unfavourable movements in the exchange rate.
        ///
        /// The sender will be provided with liquidity tokens representing their share of the
        /// liquidity pool for the given asset pair; the number of tokens provided can be
        /// determined by looking at the raised `LiquidityAdded` event. These tokens can be
        /// redeemed for the underlying assets in the pool by calling `remove_liquidity`.
        #[pallet::weight(10_000)] // TODO
        #[transactional]
        pub fn add_liquidity(
            origin: OriginFor<T>,
            asset_a: AssetIdOf<T>,
            min_amount_a: AssetBalanceOf<T>,
            max_amount_a: AssetBalanceOf<T>,
            asset_b: AssetIdOf<T>,
            min_amount_b: AssetBalanceOf<T>,
            max_amount_b: AssetBalanceOf<T>,
        ) -> DispatchResult {
            let sender = ensure_signed(origin)?;

            let asset_pair = make_asset_pair::<T>(asset_a, asset_b)?;
            let total_liquidity = TotalLiquidity::<T>::get(asset_pair);
            let pool_account = get_pool_account::<T>(asset_pair);

            let pool_amount_a = T::Fungibles::balance(asset_a, &pool_account);
            let pool_amount_b = T::Fungibles::balance(asset_b, &pool_account);

            let (added_liquidity, amount_a, amount_b) = if total_liquidity.is_zero() {
                // The sender is the first liquidity provider. The value we choose for
                // added_liquidity here is somewhat arbitrary.
                (
                    max(max_amount_a, max_amount_b)
                        .saturating_mul(T::InitialLiquidityPerAssetUnit::get()),
                    max_amount_a,
                    max_amount_b,
                )
            } else {
                // There is already some liquidity in the pool. An equivalent value of each asset
                // must be added, using the current exchange rate. Figure out which of max_amount_a
                // and max_amount_b is the least valuable, and determine the added liquidity from
                // that.
                let added_liquidity =
                    if mul(max_amount_a, pool_amount_b)? < mul(max_amount_b, pool_amount_a)? {
                        // pool_amount_a=0 would imply the pool is empty despite the total
                        // liquidity being non-zero
                        mul_div_floor(max_amount_a, total_liquidity, pool_amount_a)?
                    } else {
                        mul_div_floor(max_amount_b, total_liquidity, pool_amount_b)?
                    };

                // Determine the actual amounts to add to the pool. We round down above and up here
                // to favour the existing liquidity providers over the sender of this transaction.
                let amount_a = mul_div_ceil(added_liquidity, pool_amount_a, total_liquidity)?;
                let amount_b = mul_div_ceil(added_liquidity, pool_amount_b, total_liquidity)?;

                (added_liquidity, amount_a, amount_b)
            };

            // Abort the transaction if the sender would not add enough of each asset (these checks
            // exist to protect the sender against adding liquidity to a pool when the exchange
            // rate has moved too far from what they expect).
            ensure!(amount_a >= min_amount_a, Error::<T>::UnexpectedExchangeRate);
            ensure!(amount_b >= min_amount_b, Error::<T>::UnexpectedExchangeRate);

            // Transfer the assets to the pool. Note we might end up adding a bit more than we
            // thought if the source account would otherwise end up with a balance between 0 and
            // the minimum. This is harmless, but we do take care to report it properly in the
            // LiquidityAdded event...
            let amount_a =
                T::Fungibles::transfer(asset_a, &sender, &pool_account, amount_a, false)?;
            let pool_amount_a = add(pool_amount_a, amount_a)?;
            let amount_b =
                T::Fungibles::transfer(asset_b, &sender, &pool_account, amount_b, false)?;
            let pool_amount_b = add(pool_amount_b, amount_b)?;

            // Credit the sender with the added liquidity
            let total_liquidity = add(total_liquidity, added_liquidity)?;
            TotalLiquidity::<T>::set(asset_pair, total_liquidity);
            let sender_liquidity = Liquidity::<T>::get(&sender, asset_pair);
            let sender_liquidity = add(sender_liquidity, added_liquidity)?;
            Liquidity::<T>::set(&sender, asset_pair, sender_liquidity);

            // Check the sender added a sufficient amount of each asset
            ensure!(
                mul_div_floor(pool_amount_a, sender_liquidity, total_liquidity)? >=
                    Self::get_min_pool_amount(asset_a)?,
                Error::<T>::InsufficientPoolAmount
            );
            ensure!(
                mul_div_floor(pool_amount_b, sender_liquidity, total_liquidity)? >=
                    Self::get_min_pool_amount(asset_b)?,
                Error::<T>::InsufficientPoolAmount
            );

            Self::deposit_event(Event::LiquidityAdded {
                who: sender,
                asset_a,
                amount_a,
                asset_b,
                amount_b,
                liquidity: added_liquidity,
            });

            Ok(())
        }

        /// Redeem liquidity tokens for an asset pair. The share of the liquidity pool represented
        /// by the tokens will be transferred back to the sender.
        ///
        /// Note this transaction does not perform any exchange rate checks as the sender always
        /// benefits from any deviations from the true rate.
        #[pallet::weight(10_000)] // TODO
        #[transactional]
        pub fn remove_liquidity(
            origin: OriginFor<T>,
            asset_a: AssetIdOf<T>,
            asset_b: AssetIdOf<T>,
            liquidity: LiquidityBalanceOf<T>,
        ) -> DispatchResult {
            let sender = ensure_signed(origin)?;

            let asset_pair = make_asset_pair::<T>(asset_a, asset_b)?;
            let total_liquidity = TotalLiquidity::<T>::get(asset_pair);
            let pool_account = get_pool_account::<T>(asset_pair);

            let pool_amount_a = T::Fungibles::balance(asset_a, &pool_account);
            let pool_amount_b = T::Fungibles::balance(asset_b, &pool_account);

            let amount_a = mul_div_floor(liquidity, pool_amount_a, total_liquidity)?;
            let amount_b = mul_div_floor(liquidity, pool_amount_b, total_liquidity)?;

            // Debit the removed liquidity from the sender's account
            let total_liquidity = sub(total_liquidity, liquidity)?;
            if total_liquidity.is_zero() {
                TotalLiquidity::<T>::remove(asset_pair);
            } else {
                TotalLiquidity::<T>::set(asset_pair, total_liquidity);
            }
            let sender_liquidity = Liquidity::<T>::get(&sender, asset_pair);
            let sender_liquidity = sub(sender_liquidity, liquidity)?;
            if sender_liquidity.is_zero() {
                Liquidity::<T>::remove(&sender, asset_pair);
            } else {
                Liquidity::<T>::set(&sender, asset_pair, sender_liquidity);
            }

            // If the total liquidity after the removal is non-zero, we want to keep the pool
            // accounts alive...
            let keep_alive = !total_liquidity.is_zero();

            // Possibly reduce the transferred amounts to avoid leaving the pool with less than the
            // minimum balance of either asset
            let amount_a =
                min(amount_a, T::Fungibles::reducible_balance(asset_a, &pool_account, keep_alive));
            let amount_b =
                min(amount_b, T::Fungibles::reducible_balance(asset_b, &pool_account, keep_alive));

            // Transfer the assets to the sender
            let amount_a =
                T::Fungibles::transfer(asset_a, &pool_account, &sender, amount_a, keep_alive)?;
            let pool_amount_a = sub(pool_amount_a, amount_a)?;
            let amount_b =
                T::Fungibles::transfer(asset_b, &pool_account, &sender, amount_b, keep_alive)?;
            let pool_amount_b = sub(pool_amount_b, amount_b)?;

            // Check the sender left a sufficient amount of each asset (note that removing all of
            // your liquidity is always fine)
            if !sender_liquidity.is_zero() {
                ensure!(
                    mul_div_floor(pool_amount_a, sender_liquidity, total_liquidity)? >=
                        Self::get_min_pool_amount(asset_a)?,
                    Error::<T>::InsufficientPoolAmount
                );
                ensure!(
                    mul_div_floor(pool_amount_b, sender_liquidity, total_liquidity)? >=
                        Self::get_min_pool_amount(asset_b)?,
                    Error::<T>::InsufficientPoolAmount
                );
            }

            Self::deposit_event(Event::LiquidityRemoved {
                who: sender,
                asset_a,
                amount_a,
                asset_b,
                amount_b,
                liquidity,
            });

            Ok(())
        }

        /// Exchange a given amount of one asset for an equivalent value of another asset, using
        /// the current exchange rate.
        ///
        /// To protect the sender against unfavourable movements in the exchange rate, if the
        /// equivalent value is less than `min_dest_amount`, the transaction is aborted.
        ///
        /// A fixed percentage fee is charged and added to the liquidity pool for the asset pair.
        #[pallet::weight(10_000)] // TODO
        #[transactional]
        pub fn exchange(
            origin: OriginFor<T>,
            source_asset: AssetIdOf<T>,
            source_amount: AssetBalanceOf<T>,
            dest_asset: AssetIdOf<T>,
            min_dest_amount: AssetBalanceOf<T>,
        ) -> DispatchResult {
            let sender = ensure_signed(origin)?;

            let asset_pair = make_asset_pair::<T>(source_asset, dest_asset)?;
            let pool_account = get_pool_account::<T>(asset_pair);

            let pool_source_amount = T::Fungibles::balance(source_asset, &pool_account);
            let pool_dest_amount = T::Fungibles::balance(dest_asset, &pool_account);
            ensure!(!pool_source_amount.is_zero(), Error::<T>::NoLiquidity);
            ensure!(!pool_dest_amount.is_zero(), Error::<T>::NoLiquidity);

            let source_fee = T::ExchangeFee::get().mul_ceil(source_amount);
            let new_pool_source_amount = add(pool_source_amount, source_amount)?;
            let new_pool_source_amount_less_fee = sub(new_pool_source_amount, source_fee)?;

            // We want to preserve the product of pool_source_amount and pool_dest_amount when
            // performing the exchange, then add the fee to the pool.
            let new_pool_dest_amount = mul_div_ceil(
                pool_source_amount,
                pool_dest_amount,
                new_pool_source_amount_less_fee,
            )?;
            let dest_amount = sub(pool_dest_amount, new_pool_dest_amount)?;

            // Possibly reduce dest_amount to avoid leaving the pool with less than the minimum
            // balance of the destination asset
            let dest_amount =
                min(dest_amount, T::Fungibles::reducible_balance(dest_asset, &pool_account, true));

            // Abort the transaction if the sender would not receive enough
            ensure!(dest_amount >= min_dest_amount, Error::<T>::UnexpectedExchangeRate);

            // Transfer the assets to/from the sender. Note we might transfer more than expected to
            // the pool if the source account would otherwise end up with a balance between 0 and
            // the minimum. This is harmless, but we do take care to report it properly in the
            // Exchanged event. Possibly we should handle this before calculating dest_amount but
            // it doesn't really matter.
            let source_amount =
                T::Fungibles::transfer(source_asset, &sender, &pool_account, source_amount, false)?;
            let dest_amount =
                T::Fungibles::transfer(dest_asset, &pool_account, &sender, dest_amount, true)?;

            Self::deposit_event(Event::Exchanged {
                who: sender,
                source_asset,
                source_amount,
                dest_asset,
                dest_amount,
            });

            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        pub fn get_min_pool_amount(
            asset: AssetIdOf<T>,
        ) -> Result<AssetBalanceOf<T>, ArithmeticError> {
            let multiple = T::PoolMinAmountMultiple::get();
            T::Fungibles::minimum_balance(asset)
                .checked_mul(&multiple)
                .ok_or(ArithmeticError::Overflow)
        }

        /// Returns the amount of each asset in the liquidity pool for the asset pair.
        ///
        /// The ratio of these is the current exchange rate (this is specifically a property of the
        /// constant product CFMM). `(0, 0)` is returned if there is no liquidity pool (in which
        /// case it is impossible to exchange `asset_a` for `asset_b` or vice-versa).
        pub fn get_exchange_rate(
            asset_a: AssetIdOf<T>,
            asset_b: AssetIdOf<T>,
        ) -> (AssetBalanceOf<T>, AssetBalanceOf<T>) {
            if let Ok(asset_pair) = make_asset_pair::<T>(asset_a, asset_b) {
                let pool_account = get_pool_account::<T>(asset_pair);

                let pool_amount_a = T::Fungibles::balance(asset_a, &pool_account);
                let pool_amount_b = T::Fungibles::balance(asset_b, &pool_account);

                (pool_amount_a, pool_amount_b)
            } else {
                // Invalid asset pair, no liquidity pool
                (0u32.into(), 0u32.into())
            }
        }
    }
}
