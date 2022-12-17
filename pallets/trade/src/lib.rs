#![cfg_attr(not(feature = "std"), no_std)]

mod linked_price_list;
pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
    use frame_support::{
        pallet_prelude::*,
        sp_runtime::traits::{Hash, Bounded, AtLeast32BitUnsigned, Zero},
        traits::{Randomness, Currency},
    };
	use scale_info::TypeInfo;
    use frame_system::pallet_prelude::*;
    use crate::linked_price_list::{PriceItem, PriceList};

    #[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
    pub trait Config: frame_system::Config + pallet_tokens::Config{
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>; 
        type Currency: Currency<Self::AccountId>;
        type TradeRandom: Randomness<Self::Hash, Self::BlockNumber>;
        type Price: Parameter + Default + Member + Bounded + AtLeast32BitUnsigned + Copy + From<u128> + Into<u128>;
        type PriceFactor: Get<u128>;
        type BlocksPerDay: Get<u32>;
        type OpenedOrdersArrayCap: Get<u8>;
        type ClosedOrdersArrayCap: Get<u8>;
	}

    type Balance<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

     /* struct TradePair để quản lý các cặp trade_pair
        Ví dụ: cặp BTC/BUSD
        => Trong đó, base_token là BTC, và quote_token là BUSD 
    */ 
    #[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
	#[scale_info(skip_type_params(T))]
    pub struct TradePair<T: Config> {
        hash: T::Hash,                              // trade_pair_hash
        base: T::Hash,                              // base_token_hash       
        quote: T::Hash,                             // quote_token_hash

        latest_matched_price: Option<T::Price>,     // giá khớp lệnh gần nhất

        one_day_trade_volume: Balance<T>,           // tổng volume 24h
        one_day_highest_price: Option<T::Price>,    // giá cao nhất 24h
        one_day_lowest_price: Option<T::Price>,     // giá thấp nhất 24h
    }

    // Loại order gồm: Buy hoặc Sell
    #[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
    pub enum OrderType {
        Buy,
        Sell
    }

    // Theo dõi trạng thái của order
    #[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
    pub enum OrderStatus {
        Created,                // Khởi tạo
        PartialFilled,          // Khớp 1 phần
        Filled,                 // Khớp hết
        Canceled                // Hủy
    }

    // struct LimitOrder để quản lý các lệnh Limit
    #[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
	#[scale_info(skip_type_params(T))]
    pub struct LimitOrder<T: Config>{
        pub hash: T::Hash,                      // order_hash
        pub base: T::Hash,                      // base_token_hash
        pub quote: T::Hash,                     // quote_token_hash

        pub owner: T::AccountId,                // người tạo lệnh
        pub price: T::Price,                    // mức giá đặt
        pub sell_amount: Balance<T>,            // tổng lượng bán
        pub buy_amount: Balance<T>,             // tổng lượng mua
        pub remained_sell_amount: Balance<T>,   // lượng bán còn lại
        pub remained_buy_amount: Balance<T>,    // lượng mua còn lại

        pub otype: OrderType,                   // Buy / Sell
        pub status: OrderStatus                 // trạng thái lệnh
    }
    impl<T: Config> LimitOrder<T> {
        fn new(base: T::Hash, quote: T::Hash, owner: T::AccountId, price: T::Price, sell_amount:Balance<T>, buy_amount: Balance<T>, otype: OrderType) -> Self {
            // create order_hash
            let nonce = <Nonce<T>>::get();
            let random = T::TradeRandom::random_seed().0;
            let hash = (random, base, quote, owner.clone(), price, sell_amount, buy_amount, otype.clone(), nonce, frame_system::Pallet::<T>::block_number()).using_encoded(T::Hashing::hash);
            
            LimitOrder { hash, base, quote, owner, price, sell_amount, buy_amount, remained_sell_amount: sell_amount, remained_buy_amount: buy_amount, otype, status: OrderStatus::Created }
        }

        // check trạng thái order đã finish => đã Filled hoặc Canceled
        fn is_finished(&self) -> bool {
            self.status == OrderStatus::Filled && self.remained_buy_amount == Zero::zero() ||
            self.status == OrderStatus::Filled && self.remained_sell_amount == Zero::zero() ||
            self.status == OrderStatus::Canceled
        }
    }

    // struct Trade để quản lý các giao dịch khớp lệnh
    #[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
	#[scale_info(skip_type_params(T))]
    pub struct Trade<T: Config> {
        hash: T::Hash,                  // trade_hash
        base: T::Hash,                  // base_token_hash
        quote: T::Hash,                 // quote_token_hash

        buyer: T::AccountId,            // người bán => người sở hữu base_token
        seller: T::AccountId,           // người mua => người sở hữu quote_token
        maker: T::AccountId,            // người đặt lệnh trước
        taker: T::AccountId,            // người khớp lệnh
        otype: OrderType,               // taker order's type
        price: T::Price,                // maker order's price
        base_amount: Balance<T>,        // số lượng base_token giao dịch
        quote_amount: Balance<T>        // số lượng quote_token giao dịch
    }
    impl<T: Config> Trade<T> {
        fn new(base: T::Hash, quote: T::Hash, maker_order: &LimitOrder<T>, taker_order: &LimitOrder<T>, base_amount: Balance<T>, quote_amount: Balance<T>) -> Self {
            // cretae trade hash
            let nonce = <Nonce<T>>::get();
            let random = T::TradeRandom::random_seed().0;
            let hash = (random, frame_system::Pallet::<T>::block_number(), nonce, 
                maker_order.hash, maker_order.remained_sell_amount, maker_order.owner.clone(), 
                taker_order.hash, taker_order.remained_sell_amount, taker_order.owner.clone()).using_encoded(T::Hashing::hash);

            let new_nonce = nonce.checked_add(1);
            if let Some(n) = new_nonce { <Nonce<T>>::put(n) }

            // check thằng nào bán, thằng nào mua => otype của maker và taker phải khác nhau
            let buyer;
            let seller;
            if taker_order.otype == OrderType::Buy {
                buyer = taker_order.owner.clone();
                seller = maker_order.owner.clone();
            } else {
                buyer = maker_order.owner.clone();
                seller = taker_order.owner.clone();
            }

            Trade { hash, base, quote, buyer, seller, maker: maker_order.owner.clone(), taker: taker_order.owner.clone(), otype: taker_order.otype.clone(), price: maker_order.price, base_amount, quote_amount }
        }
    }

    // type OrderItem<T> = PriceItem<<T as frame_system::Config>::Hash, <T as Config>::Price, Balance<T>>;
    // type OrderList<T> = PriceList<T, LinkedItemList<T>, <T as frame_system::Config>::Hash, <T as Config>::Price, Balance<T>>;

    // #[pallet::storage]
    // #[pallet::getter(fn linked_item)]
    // pub type LinkedItemList<T: Config> = StorageMap<_, Blake2_128Concat, (T::Hash, Option<T::Price>), OrderItem<T>>; 

    #[pallet::storage]
	#[pallet::getter(fn nonce)]
	pub type Nonce<T: Config> = StorageValue<_, u128, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pairs)]
    // Mapping tp_hash => tp
    pub type TradePairs<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, TradePair<T>>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_hash_by_base_quote)]
    // Mapping (base, quote) -> tp_hash
    pub type TradePairsHashByBaseQuote<T: Config> = StorageMap<_, Blake2_128Concat, (T::Hash, T::Hash), T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_hash_by_index)]
    // Mapping index => tp_hash
    pub type TradePairsHashByIndex<T: Config> = StorageMap<_, Blake2_128Concat, u64, T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_index)]
    pub type TradePairsIndex<T: Config> = StorageValue<_, u64, ValueQuery>; 

    #[pallet::storage]
    #[pallet::getter(fn orders)]
    // Mapping order_hash => LimitOrder
    pub type Orders<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, LimitOrder<T>>;

    #[pallet::storage]
    #[pallet::getter(fn owned_orders)]
    // Mapping (account_id, index) -> order_hash 
    pub type OwnedOrders<T: Config> = StorageMap<_, Blake2_128Concat, (T::AccountId, u64), T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn owned_orders_index)]
    // Mapping account_id -> index 
    pub type OwnedOrdersIndex<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, u64, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn order_owned_trades)]
    // Mapping (order_hash, index) => trade_hash
    pub type OrderOwnedTrades<T: Config> = StorageMap<_, Blake2_128Concat, (T::Hash, u64), T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn order_owned_trades_index)]
    // Mapping order_hash => index
    pub type OrderOwnedTradesIndex<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, u64, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_owned_order)]
    // Mapping (tp_hash, index) => order_hash
    pub type TradePairOwnedOrders<T: Config> = StorageMap<_, Blake2_128Concat, (T::Hash, u64), T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_owned_order_index)]
    // Mapping tp_hash => index
    pub type TradePairOwnedOrdersIndex<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, u64, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn trades)]
    pub type Trades<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, Trade<T>>;

    #[pallet::storage]
    #[pallet::getter(fn owned_trades)]
    // Mapping (account_id, index) => trade_hash
    pub type OwnedTrades<T: Config> = StorageMap<_, Blake2_128Concat, (T::AccountId, u64), T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn owned_trades_index)]
    // Mapping account_id => index
    pub type OwnedTradesIndex<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, u64, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn owned_tp_trades)]
    // Mapping (account_id, tp_hash, index) => trade_hash
    pub type OwnedTPTrades<T: Config> = StorageMap<_, Blake2_128Concat, (T::AccountId, T::Hash, u64), T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn owned_tp_trades_index)]
    // Mapping (account_id, tp_hash) => index
    pub type OwnedTPTradesIndex<T: Config> = StorageMap<_, Blake2_128Concat, (T::AccountId, T::Hash), u64, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn owned_tp_opened_orders)]
    // Mapping (account_id, tp_hash) => orders
    pub type OwnedTPOpenedOrders<T: Config> = StorageMap<_, Blake2_128Concat, (T::AccountId, T::Hash), Vec<T::Hash>>;

    #[pallet::storage]
    #[pallet::getter(fn owned_tp_closed_orders)]
    // Mapping (aacount_id, tp_hash) => orders
    pub type OwnedTPClosedOrders<T: Config> = StorageMap<_, Blake2_128Concat, (T::AccountId, T::Hash), Vec<T::Hash>>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_owned_trades)]
    // Mapping (tp_hash, index) => trade_hash
    pub type TradePairOwnedTrades<T: Config> = StorageMap<_, Blake2_128Concat, (T::Hash, u64), T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_owned_trades_index)]
    // Mapping tp_hash => index
    pub type TradePairOwnedTradesIndex<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, u64, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_trade_data_bucket)]
    // Mapping (tp_hash, blocknumber) => 
    pub type TPTradeDataBucket<T: Config> = StorageMap<_, Blake2_128Concat, (T::Hash, T::BlockNumber), (Balance<T>, Option<T::Price>, Option<T::Price>)>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_trade_price_bucket)]
    // Mapping tp_hash => bucket
    pub type TPTradePriceBucket<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, (Vec<Option<T::Price>>, Vec<Option<T::Price>>)>;

    #[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config>{}

	#[pallet::error]
	pub enum Error<T> {}

    #[pallet::call]
    impl<T: Config> Pallet<T> {}

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {}

    impl<T: Config> Pallet<T> {}
}