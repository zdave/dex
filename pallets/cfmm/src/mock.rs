use crate as pallet_cfmm;
use frame_support::{
    parameter_types,
    traits::{ConstU16, ConstU32, ConstU64, StorageMapShim},
    PalletId,
};
use frame_system as system;
use frame_system::EnsureRoot;
use sp_core::H256;
use sp_runtime::{
    testing::Header,
    traits::{BlakeTwo256, IdentityLookup},
    Permill,
};

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

type AccountId = u128; // Should be at least 128 bits with 32-bit AssetId to avoid get_pool_account collisions
type Balance = u32;
type AssetBalance = u32;
type AssetId = u32;

// Configure a mock runtime to test the pallet.
frame_support::construct_runtime!(
    pub enum Test where
        Block = Block,
        NodeBlock = Block,
        UncheckedExtrinsic = UncheckedExtrinsic,
    {
        System: frame_system,
        Balances: pallet_balances,
        Assets: pallet_assets,
        Cfmm: pallet_cfmm,
    }
);

impl system::Config for Test {
    type BaseCallFilter = frame_support::traits::Everything;
    type BlockWeights = ();
    type BlockLength = ();
    type DbWeight = ();
    type Origin = Origin;
    type Call = Call;
    type Index = u64;
    type BlockNumber = u64;
    type Hash = H256;
    type Hashing = BlakeTwo256;
    type AccountId = AccountId;
    type Lookup = IdentityLookup<AccountId>;
    type Header = Header;
    type Event = Event;
    type BlockHashCount = ConstU64<250>;
    type Version = ();
    type PalletInfo = PalletInfo;
    type AccountData = ();
    type OnNewAccount = ();
    type OnKilledAccount = ();
    type SystemWeightInfo = ();
    type SS58Prefix = ConstU16<42>;
    type OnSetCode = ();
    type MaxConsumers = frame_support::traits::ConstU32<16>;
}

impl pallet_balances::Config for Test {
    type MaxLocks = ();
    type MaxReserves = ();
    type ReserveIdentifier = ();
    type Balance = Balance;
    type Event = Event;
    type DustRemoval = ();
    type ExistentialDeposit = ConstU32<1>;
    type AccountStore = StorageMapShim<
        pallet_balances::Account<Test>,
        frame_system::Provider<Test>,
        AccountId,
        pallet_balances::AccountData<Balance>,
    >;
    type WeightInfo = pallet_balances::weights::SubstrateWeight<Test>;
}

impl pallet_assets::Config for Test {
    type Event = Event;
    type Balance = AssetBalance;
    type AssetId = AssetId;
    type Currency = Balances;
    type ForceOrigin = EnsureRoot<AccountId>;
    type AssetDeposit = ();
    type AssetAccountDeposit = ();
    type MetadataDepositBase = ();
    type MetadataDepositPerByte = ();
    type ApprovalDeposit = ();
    type StringLimit = ConstU32<32>;
    type Freezer = ();
    type Extra = ();
    type WeightInfo = pallet_assets::weights::SubstrateWeight<Test>;
}

parameter_types!(
    pub const CfmmPalletId: PalletId = PalletId(*b"cfmm____");
    pub const CfmmPoolMinAmountMultiple: AssetBalance = 10;
    pub const CfmmInitialLiquidityPerAssetUnit: AssetBalance = 10;
    pub const CfmmExchangeFee: Permill = Permill::from_percent(10);
);

impl pallet_cfmm::Config for Test {
    type Event = Event;
    type PalletId = CfmmPalletId;
    type AssetId = AssetId;
    type AssetBalance = AssetBalance;
    type Fungibles = Assets;
    type PoolMinAmountMultiple = CfmmPoolMinAmountMultiple;
    type InitialLiquidityPerAssetUnit = CfmmInitialLiquidityPerAssetUnit;
    type ExchangeFee = CfmmExchangeFee;
}

// Build genesis storage according to the mock runtime.
pub fn new_test_ext() -> sp_io::TestExternalities {
    system::GenesisConfig::default().build_storage::<Test>().unwrap().into()
}
