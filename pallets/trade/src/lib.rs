#![cfg_attr(not(feature = "std"), no_std)]

mod linked_price_list;
pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
    use frame_support::{
        pallet_prelude::*,
        sp_runtime::{
            traits::{Hash, Bounded, AtLeast32Bit, Zero, CheckedSub},
            ArithmeticError
        },
        traits::{Randomness},
        sp_std::{
            fmt::Debug,
            convert::{TryFrom, TryInto},
            ops::Not,
        },
        inherent::Vec
    };
    use scale_info::TypeInfo;
    use frame_system::pallet_prelude::*;
    use sp_core::U256;
    use crate::linked_price_list::{PriceItem, PriceList};

    #[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
    pub trait Config: frame_system::Config + pallet_tokens::Config + Debug{
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>; 
        type Price: Parameter + Default + Member + Bounded + AtLeast32Bit + Copy + From<u128> + Into<u128>;
        type TradeRandom: Randomness<Self::Hash, Self::BlockNumber>;
        type PriceFactor: Get<u128>;                // 100_000_000
        type BlocksPerDay: Get<u32>;                // 6 * 60 * 24
        type OpenedOrdersArrayCap: Get<u8>;         // 20
        type ClosedOrdersArrayCap: Get<u8>;         // 100
	}

    type Balance<T> = pallet_tokens::Balance<T>;

     /* struct TradePair để quản lý các cặp trade_pair
     Ở trong program này thì cặp trade pair sẽ theo hướng stable_coin/coin_cần_mua
        Ví dụ: cặp BUSD/BTC
        => Trong đó, base_token là BUSD, và quote_token là BTC 
        => Khi đặt lệnh BUY => là mình đang mua BTC
        => Khi đặt lệnh SELL => là mình đang bán BTC
    */ 
    #[derive(Encode, Decode, Clone, PartialEq, Eq, TypeInfo, RuntimeDebug)]
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
    #[derive(Encode, Decode, Clone, Copy, PartialEq, Eq, TypeInfo, RuntimeDebug)]
    pub enum OrderType {
        Buy,
        Sell
    }
    impl Not for OrderType { // trait Not này có tác dụng áp dụng toán tử ! cho type được impl, ví dụ: !Buy = Sell và ngược lại 
        type Output = Self;

        fn not(self) -> Self::Output {
            match self {
                OrderType::Buy => OrderType::Sell,
                OrderType::Sell => OrderType::Buy 
            }
        }
    }

    #[derive(Encode, Decode, Clone, Copy, PartialEq, Eq, TypeInfo, RuntimeDebug)]
    pub enum OrderOpt {
        Limit,
        Market
    }

    // Theo dõi trạng thái của order
    #[derive(Encode, Decode, Clone, Copy, PartialEq, Eq, TypeInfo, RuntimeDebug)]
    pub enum OrderStatus {
        Created,                // Khởi tạo
        PartialFilled,          // Khớp 1 phần
        Filled,                 // Khớp hết
        Canceled                // Hủy
    }

    // struct Order để quản lý các lệnh Limit
    #[derive(Encode, Decode, Clone, PartialEq, Eq, TypeInfo, RuntimeDebug)]
	#[scale_info(skip_type_params(T))]
    pub struct Order<T: Config>{
        pub hash: T::Hash,                      // order_hash
        pub base: T::Hash,                      // base_token_hash
        pub quote: T::Hash,                     // quote_token_hash

        pub owner: T::AccountId,                // người tạo lệnh
        pub price: T::Price,                    // mức giá đặt
        pub sell_amount: Balance<T>,            // tổng lượng bán
        pub buy_amount: Balance<T>,             // tổng lượng mua
        pub remained_sell_amount: Balance<T>,   // lượng bán còn lại
        pub remained_buy_amount: Balance<T>,    // lượng mua còn lại

        pub oopt: OrderOpt,                     // Limit / Market
        pub otype: OrderType,                   // Buy / Sell
        pub status: OrderStatus                 // trạng thái lệnh
    }
    impl<T: Config> Order<T> {
        fn new(base: T::Hash, quote: T::Hash, owner: T::AccountId, price: T::Price, sell_amount:Balance<T>, buy_amount: Balance<T>, oopt: OrderOpt, otype: OrderType) -> Self {
            // create order_hash
            let nonce = <Nonce<T>>::get();
            let random = T::TradeRandom::random_seed().0;
            let hash = (base, quote, owner.clone(), price, sell_amount, buy_amount, oopt, otype, 
                                             random, nonce, frame_system::Pallet::<T>::block_number()).using_encoded(T::Hashing::hash);
            
            Order { hash, base, quote, owner, price, sell_amount, buy_amount, remained_sell_amount: sell_amount, remained_buy_amount: buy_amount, oopt, otype, status: OrderStatus::Created }
        }

        // check trạng thái order đã finish => đã Filled hoặc Canceled
        pub fn is_finished(&self) -> bool {
            self.status == OrderStatus::Filled && self.remained_buy_amount == Zero::zero() ||
            self.status == OrderStatus::Canceled
        }
    }

    // struct Trade để quản lý các giao dịch khớp lệnh
    #[derive(Encode, Decode, Clone, PartialEq, Eq, TypeInfo, RuntimeDebug)]
	#[scale_info(skip_type_params(T))]
    pub struct Trade<T: Config> {
        hash: T::Hash,                  // trade_hash
        base: T::Hash,                  // base_token_hash
        quote: T::Hash,                 // quote_token_hash

        buyer: T::AccountId,            // người bán => người sở hữu base_token
        seller: T::AccountId,           // người mua => người sở hữu quote_token
        maker: T::AccountId,            // lệnh được đặt trước
        taker: T::AccountId,            // lệnh đặt sau mà khớp lệnh vói maker
        otype: OrderType,               // taker order's type
        price: T::Price,                // maker order's price
        base_amount: Balance<T>,        // số lượng base_token giao dịch
        quote_amount: Balance<T>        // số lượng quote_token giao dịch
    }
    impl<T: Config> Trade<T> {
        fn new(base: T::Hash, quote: T::Hash, maker_order: &Order<T>, taker_order: &Order<T>, base_amount: Balance<T>, quote_amount: Balance<T>) -> Self {
            // create trade hash
            let nonce = <Nonce<T>>::get();
            let random = T::TradeRandom::random_seed().0;
            let hash = (random, frame_system::Pallet::<T>::block_number(), nonce, 
                maker_order.hash, maker_order.remained_sell_amount, maker_order.owner.clone(), 
                taker_order.hash, taker_order.remained_sell_amount, taker_order.owner.clone()).using_encoded(T::Hashing::hash);

            let new_nonce = nonce.checked_add(1);
            if let Some(n) = new_nonce { <Nonce<T>>::put(n) }

            // check lệnh nào bán, lệnh nào mua => Nếu taker là bán thì maker là mua và ngược lại
            let buyer;
            let seller;
            if taker_order.otype == OrderType::Buy {
                buyer = taker_order.owner.clone();
                seller = maker_order.owner.clone();
            } else {
                buyer = maker_order.owner.clone();
                seller = taker_order.owner.clone();
            }

            Trade { hash, base, quote, buyer, seller, maker: maker_order.owner.clone(), taker: taker_order.owner.clone(), otype: taker_order.otype, price: maker_order.price, base_amount, quote_amount }
        }
    }

    type OrderItem<T> = PriceItem<<T as frame_system::Config>::Hash, <T as Config>::Price, Balance<T>>;
    type OrderList<T> = PriceList<T, LinkedItemList<T>, <T as frame_system::Config>::Hash, <T as Config>::Price, Balance<T>>;

    #[pallet::storage]
    #[pallet::getter(fn linked_item)]
    // Là type S bên file linked_price_list 
    pub type LinkedItemList<T: Config> = StorageMap<_, Blake2_128Concat, (T::Hash, Option<T::Price>), OrderItem<T>>; 

    #[pallet::storage]
	#[pallet::getter(fn nonce)]
	pub type Nonce<T: Config> = StorageValue<_, u128, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pairs)]
    // tp_hash => tp
    pub type TradePairs<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, TradePair<T>>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_hash_by_base_quote)]
    // (base, quote) -> tp_hash
    pub type TradePairsHashByBaseQuote<T: Config> = StorageMap<_, Blake2_128Concat, (T::Hash, T::Hash), T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_hash_by_index)]
    // index => tp_hash
    pub type TradePairsHashByIndex<T: Config> = StorageMap<_, Blake2_128Concat, u64, T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_index)]
    pub type TradePairsIndex<T: Config> = StorageValue<_, u64, ValueQuery>; 

    #[pallet::storage]
    #[pallet::getter(fn orders)]
    // order_hash => Order
    pub type Orders<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, Order<T>>;

    #[pallet::storage]
    #[pallet::getter(fn owned_orders)]
    // (account_id, index) -> order_hash 
    pub type OwnedOrders<T: Config> = StorageMap<_, Blake2_128Concat, (T::AccountId, u64), T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn owned_orders_index)]
    // account_id -> index 
    pub type OwnedOrdersIndex<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, u64, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn order_owned_trades)]
    // (order_hash, index) => trade_hash
    pub type OrderOwnedTrades<T: Config> = StorageMap<_, Blake2_128Concat, (T::Hash, u64), T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn order_owned_trades_index)]
    // order_hash => index
    pub type OrderOwnedTradesIndex<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, u64, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_owned_order)]
    // (tp_hash, index) => order_hash
    pub type TradePairOwnedOrders<T: Config> = StorageMap<_, Blake2_128Concat, (T::Hash, u64), T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_owned_order_index)]
    // tp_hash => index
    pub type TradePairOwnedOrdersIndex<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, u64, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn trades)]
    // trade_hash => trade
    pub type Trades<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, Trade<T>>;

    #[pallet::storage]
    #[pallet::getter(fn owned_trades)]
    // (account_id, index) => trade_hash
    pub type OwnedTrades<T: Config> = StorageMap<_, Blake2_128Concat, (T::AccountId, u64), T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn owned_trades_index)]
    // account_id => index
    pub type OwnedTradesIndex<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, u64, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn owned_tp_trades)]
    // (account_id, tp_hash, index) => trade_hash
    pub type OwnedTPTrades<T: Config> = StorageMap<_, Blake2_128Concat, (T::AccountId, T::Hash, u64), T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn owned_tp_trades_index)]
    // (account_id, tp_hash) => index
    pub type OwnedTPTradesIndex<T: Config> = StorageMap<_, Blake2_128Concat, (T::AccountId, T::Hash), u64, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn owned_tp_opened_orders)]
    // (account_id, tp_hash) => orders
    pub type OwnedTPOpenedOrders<T: Config> = StorageMap<_, Blake2_128Concat, (T::AccountId, T::Hash), Vec<T::Hash>>;

    #[pallet::storage]
    #[pallet::getter(fn owned_tp_closed_orders)]
    // (aacount_id, tp_hash) => orders
    pub type OwnedTPClosedOrders<T: Config> = StorageMap<_, Blake2_128Concat, (T::AccountId, T::Hash), Vec<T::Hash>>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_owned_trades)]
    // (tp_hash, index) => trade_hash
    pub type TradePairOwnedTrades<T: Config> = StorageMap<_, Blake2_128Concat, (T::Hash, u64), T::Hash>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_owned_trades_index)]
    // tp_hash => index
    pub type TradePairOwnedTradesIndex<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, u64, ValueQuery>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_trade_data_bucket)]
    // (tp_hash, blocknumber) => (sum_of_trade_volume, highest_price, lowest_price)
    pub type TPTradeDataBucket<T: Config> = StorageMap<_, Blake2_128Concat, (T::Hash, T::BlockNumber), (Balance<T>, Option<T::Price>, Option<T::Price>)>;

    #[pallet::storage]
    #[pallet::getter(fn trade_pair_trade_price_bucket)]
    // tp_hash => (Vec<highest_price>, Vec<lowest_price>)
    pub type TPTradePriceBucket<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, (Vec<Option<T::Price>>, Vec<Option<T::Price>>)>;

    trait AddTrade{
        type A;
        type B;
        type C;
        fn add_trade(_param1: Self::A, _param2: Self::B, _param3: Self::C) -> DispatchResult;
    }    
    trait AddOrder<T: Config> {
        fn add_order(_param1: T::AccountId, _param2: T::Hash, _param3: T::Hash);
    } 
    trait RemoveOrder<T: Config> {
        fn remove_order(_param1: T::AccountId, _param2: T::Hash, _param3: T::Hash);
    } 

    // Khi có trade thành công thì nó update thông tin trade đó vào các storage
    impl<T: Config> AddTrade for OrderOwnedTrades<T> {
        type A = T::Hash;
        type B = T::Hash;
        type C = Option<T>;
        fn add_trade(order_hash: Self::A, trade_hash: Self::B, _: Self::C) -> DispatchResult{
            let index = <OrderOwnedTradesIndex<T>>::get(&order_hash);
            Self::insert((order_hash.clone(), index), trade_hash);
            let new_index = index.checked_add(1).ok_or(ArithmeticError::Overflow)?;
            <OrderOwnedTradesIndex<T>>::insert(order_hash, new_index);
            Ok(())
        }
    }
    impl<T: Config> AddTrade for OwnedTrades<T> {
        type A = T::AccountId;
        type B = T::Hash;
        type C = Option<T>;
        fn add_trade(account_id: Self::A, trade_hash: Self::B, _: Self::C) -> DispatchResult{
            let index = <OwnedTradesIndex<T>>::get(&account_id);
            Self::insert((account_id.clone(), index), trade_hash);
            let new_index = index.checked_add(1).ok_or(ArithmeticError::Overflow)?;
            <OwnedTradesIndex<T>>::insert(account_id, new_index);
            Ok(())
        }
    }
    impl<T: Config> AddTrade for TradePairOwnedTrades<T> {
        type A = T::Hash;
        type B = T::Hash;
        type C = Option<T>;
        fn add_trade(tp_hash: Self::A, trade_hash: Self::B, _: Self::C) -> DispatchResult{
            let index = <TradePairOwnedTradesIndex<T>>::get(&tp_hash);
            Self::insert((tp_hash.clone(), index), trade_hash);
            let new_index = index.checked_add(1).ok_or(ArithmeticError::Overflow)?;
            <TradePairOwnedTradesIndex<T>>::insert(tp_hash, new_index);
            Ok(())
        }
    }
    impl<T: Config> AddTrade for OwnedTPTrades<T> {
        type A = T::AccountId;
        type B = T::Hash;
        type C = T::Hash;
        fn add_trade(account_id: Self::A, tp_hash: Self::B, trade_hash: Self::C) -> DispatchResult{
            let index = <OwnedTPTradesIndex<T>>::get((account_id.clone(), tp_hash));
            Self::insert((account_id.clone(), tp_hash, index), trade_hash);
            let new_index = index.checked_add(1).ok_or(ArithmeticError::Overflow)?;
            <OwnedTPTradesIndex<T>>::insert((account_id.clone(), tp_hash), new_index);
            Ok(())
        }
    }
    
    // Khi có một order được khởi tạo thì add nó vào opened_order
    impl<T: Config> AddOrder<T> for OwnedTPOpenedOrders<T> {
        fn add_order(account_id: T::AccountId, tp_hash: T::Hash, order_hash: T::Hash){
    
            let mut orders;
            // lấy ds orders ra nếu chưa có thì khởi tạo
            if let Some(ts) = Self::get((account_id.clone(), tp_hash)) {
                orders = ts;
            } else {
                orders = Vec::<T::Hash>::new();
            }
    
            // check order này đã tồn tại chưa, có thì bỏ qua còn chưa thì add vào ds
            match orders.iter().position(|&x| x == order_hash) {
                Some(_) => return,
                None => { // chèn vào đầu vec, nếu len của ds = opened_cap thì mình xóa phần tử cuối
                    orders.insert(0, order_hash);
                    if orders.len() == T::OpenedOrdersArrayCap::get() as usize {
                        orders.pop();
                    }
                    // update lại ds orders
                    <OwnedTPOpenedOrders<T>>::insert((account_id, tp_hash), orders);
                }
            }
        }
    }
    
    // Khi một order được hoàn tất hoặc cancel thì mình xóa khỏi opened_order và thêm vào closed_order
    impl<T: Config> RemoveOrder<T> for OwnedTPOpenedOrders<T> {
        fn remove_order(account_id: T::AccountId, tp_hash: T::Hash, order_hash: T::Hash) {
    
            let mut orders;
            if let Some(ts) = Self::get((account_id.clone(), tp_hash)) {
                orders = ts;
            } else {
                orders = Vec::<T::Hash>::new();
            }
    
            orders.retain(|&x| x != order_hash);
            <OwnedTPOpenedOrders<T>>::insert((account_id, tp_hash), orders);
        }
    }
    impl<T: Config> AddOrder<T> for OwnedTPClosedOrders<T> {
        fn add_order(account_id: T::AccountId, tp_hash: T::Hash, order_hash: T::Hash){
    
            let mut orders;
            if let Some(ts) = Self::get((account_id.clone(), tp_hash)) {
                orders = ts;
            } else {
                orders = Vec::<T::Hash>::new();
            }
    
            match orders.iter().position(|&x| x == order_hash) {
                Some(_) => return,
                None => {
                    orders.insert(0, order_hash);
                    if orders.len() == T::ClosedOrdersArrayCap::get() as usize {
                        orders.pop();
                    }
    
                    <OwnedTPClosedOrders<T>>::insert((account_id, tp_hash), orders);
                }
            }
        }
    }

    #[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config>{
        // emit khi 1 tradepair được tạo => (người tạo, tp_hash, tp)
        TradePairCreated {
            owner: T::AccountId,            
            hash: T::Hash,                  
            trade_pair: TradePair<T>            
        },
        // emit khi order được tạo => (người tạo, base_token, quote_token, order_hash, limit_order)
        OrderCreated {
            owner: T::AccountId,
            base_token: T::Hash,
            quote_token: T::Hash,
            order_hash: T::Hash,
            limit_order: Order<T>
        },
        // emit khi trade được tạo => (người tạo, base_token, quote_token, trade_hash, trade)
        TradeCreated {
            owner: T::AccountId,
            base_token: T::Hash,
            quote_token: T::Hash,
            trade_hash: T::Hash,
            trade: Trade<T>
        },
        // emit khi order cancel => (người cancel, order_hash)
        OrderCanceled {
            owner: T::AccountId,
            order_hash: T::Hash
        },
    }

	#[pallet::error]
	pub enum Error<T> {
        /// Price bounds check failed
		BoundsCheckFailed,
		BoundsCheckFailedPrice,
		BoundsCheckFailedAmount,
		BoundsCheckFailedAmount2,
		BoundsCheckFailedAmountCounter,
		/// Price length check failed
		PriceLengthCheckFailed,
		/// Number cast error
		NumberCastError,
		/// Overflow error
		OverflowError,
        /// No matching trade pair
        NoMatchingTradePair,
        /// Base equals to quote
        BaseEqualQuote,
        /// Token owner not found
        TokenOwnerNotFound,
        /// Sender not equal to base or quote owner
        SenderNotEqualToBaseOrQuoteOwner,
        /// Same trade pair with the given base and quote was already exist
        TradePairExisted,
        /// Get price error
        OrderMatchGetPriceError,
        /// Get linked list item error
        OrderMatchGetLinkedListItemError,
        /// Get order error
        OrderMatchGetOrderError,
        /// Order match substract error
        OrderMatchSubstractError,
        /// Order match order is not finish
        OrderMatchOrderIsNotFinished,
        /// No matching order
        NoMatchingOrder,
        /// Can only cancel own order
        CanOnlyCancelOwnOrder,
        /// can only cancel not finished order
        CanOnlyCancelNotFinishedOrder, 
        // can't get TPTradeDataBucket
        DataBucketOfTradePairNotExist,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::weight(1_000_000)]
        pub fn create_trade_pair(origin: OriginFor<T>, base: T::Hash, quote: T::Hash) -> DispatchResult {
            let sender = ensure_signed(origin)?;
            Self::do_create_trade_pair(sender, base, quote)
        }

        #[pallet::weight(1_000_000)]
        pub fn create_order(origin: OriginFor<T>, base: T::Hash, quote: T::Hash, oopt: OrderOpt, otype: OrderType, price: T::Price, sell_amount: Balance<T>) -> DispatchResult {
            let sender = ensure_signed(origin)?;
            if oopt == OrderOpt::Limit {
                Self::do_create_limit_order(sender, base, quote, otype, price, sell_amount)
            } else {
                Self::do_create_market_order(sender, base, quote, otype, sell_amount)
            }
        }

        // #[pallet::weight(100_000_000)]
        // pub fn create_limit_order_with_le_float(
        //     origin: OriginFor<T>,
        //     base: T::Hash,
        //     quote: T::Hash,
        //     otype: OrderType,
        //     price: Vec<u8>,
        //     sell_amount: Balance<T>
        // ) -> DispatchResult {
        //     let sender = ensure_signed(origin)?;
        //     let price = Self::price_as_vec_u8_to_x_by_100m(price)?;
        //     Self::do_create_limit_order(sender, base, quote, otype, price, sell_amount)
        // }

        #[pallet::weight(1_000_000)]
        pub fn cancel_limit_order(origin: OriginFor<T>, order_hash: T::Hash) -> DispatchResult {
            let sender = ensure_signed(origin)?;
            Self::do_cancel_limit_order(sender, order_hash)
        }
    }

    // 2 fn trong hook chủ yếu là cập nhật thông tin tổng volume, giá cao nhất, giá thấp nhất => không ảnh hưởng đến hoạt động trade
    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        // logic chạy trước khi executing transaction 
        fn on_initialize(block_number: T::BlockNumber) -> Weight{
            // get BlocksPerDay & convert to type BlockNumber
            let days: T::BlockNumber = <<T as frame_system::Config>::BlockNumber as From<_>>::from(T::BlocksPerDay::get());
            let total_weight: Weight = Zero::zero();
            // nếu số block chưa quá 1 ngày thì return
            if block_number <= days {
                return total_weight.add(1000)
            }

            // lặp qua toàn bộ các cặp trade_pair và update lại sum_vol
            for i in 0..<TradePairsIndex<T>>::get() {
                let tp_hash = <TradePairsHashByIndex<T>>::get(i).unwrap();
                let mut tp = <TradePairs<T>>::get(tp_hash).unwrap();
                let (sum_volume, _, _) = <TPTradeDataBucket<T>>::get((tp_hash, block_number - days)).unwrap_or((Default::default(), None, None)); // sum_of_trade_volume
                tp.one_day_trade_volume -= sum_volume;
                <TradePairs<T>>::insert(tp_hash, tp);

                // update lại ds_high_price và ds_low_price
                let (mut high_pri_vec, mut low_pri_vec) = <TPTradePriceBucket<T>>::get(tp_hash).unwrap_or((Vec::<Option<T::Price>>::new(), Vec::<Option<T::Price>>::new()));
                if high_pri_vec.len() > 0 {
                    high_pri_vec.remove(0);
                }
                if low_pri_vec.len() > 0 {
                    low_pri_vec.remove(0);
                }
                <TPTradePriceBucket<T>>::insert(tp_hash, (high_pri_vec, low_pri_vec));
            }

            return total_weight.add(500_000)
        }

        // logic chạy sau khi transaction execute khi chuẩn bị finalize block
        fn on_finalize(block_number: T::BlockNumber) {
            // lặp qua hết các cặp trade_pair
            for i in 0..<TradePairsIndex<T>>::get() {
                let tp_hash = <TradePairsHashByIndex<T>>::get(i).unwrap();
                let mut tp = <TradePairs<T>>::get(tp_hash).unwrap();

                // Lấy sum_volume, giá cao nhất, giá thấp nhất, list giá cao nhất, list giá thấp nhất => cập nhật lại toàn bộ dữ liệu
                let (sum_volume, highest_price, lowest_pricce) = <TPTradeDataBucket<T>>::get((tp_hash, block_number)).unwrap_or((Default::default(), None, None));
                let (mut high_pri_vec, mut low_pri_vec) = <TPTradePriceBucket<T>>::get(tp_hash).unwrap_or((Vec::<Option<T::Price>>::new(), Vec::<Option<T::Price>>::new()));
                high_pri_vec.push(highest_price);
                low_pri_vec.push(lowest_pricce);

                let mut h_price = T::Price::min_value();
                for price in high_pri_vec.clone() {
                    if let Some(_price) = price {
                        if _price > h_price{
                            h_price = _price;
                        }
                    }
                }

                let mut l_price = T::Price::max_value();
                for price in low_pri_vec.clone() {
                    if let Some(_price) = price {
                        if _price < l_price {
                            l_price = _price;
                        }
                    }
                }

                tp.one_day_trade_volume += sum_volume;

                if h_price != T::Price::min_value() {
                    tp.one_day_highest_price = Some(h_price);
                } else {
                    tp.one_day_highest_price = None;
                }

                if l_price != T::Price::max_value() {
                    tp.one_day_lowest_price = Some(l_price);
                } else {
                    tp.one_day_lowest_price = None;
                }

                <TPTradePriceBucket<T>>::insert(tp_hash, (high_pri_vec, low_pri_vec));
                <TradePairs<T>>::insert(tp_hash, tp);
            }
        }
    }

    impl<T: Config> Pallet<T> {
        fn do_create_trade_pair(sender: T::AccountId, base: T::Hash, quote: T::Hash) -> DispatchResult {
            // check base, quote ko phải cùng 1 loại token
            ensure!(base != quote, <Error<T>>::BaseEqualQuote);

            // check base và quote đều có owner
            let base_owner = pallet_tokens::Pallet::<T>::owners(base);
            let quote_owner = pallet_tokens::Pallet::<T>::owners(quote);
            ensure!(base_owner.is_some() && quote_owner.is_some(), <Error<T>>::TokenOwnerNotFound);

            // check người tạo cặp giao dịch phải là owner 1 trong 2 token
            let base_owner = base_owner.unwrap();
            let quote_owner = quote_owner.unwrap();
            ensure!(sender == base_owner || sender == quote_owner, <Error<T>>::SenderNotEqualToBaseOrQuoteOwner);

            // check cặp giao dịch này vẫn chưa được tạo
            let bq = Self::trade_pair_hash_by_base_quote((base, quote));
            let qb = Self::trade_pair_hash_by_base_quote((quote, base));
            ensure!(!bq.is_some() && !qb.is_some(), <Error<T>>::TradePairExisted);

            // tạo random hash cho cặp tp này
            let nonce = Self::nonce();
            let random = T::TradeRandom::random_seed().0;
            let hash = (random, frame_system::Pallet::<T>::block_number(), sender.clone(), base, quote, nonce).using_encoded(T::Hashing::hash);

            let tp = TradePair { 
                hash, base, quote, 
                latest_matched_price: None, 
                one_day_trade_volume: Default::default(), 
                one_day_highest_price: None, 
                one_day_lowest_price: None 
            };

            // update_ nonce
            let new_nonce = nonce.checked_add(1).ok_or(ArithmeticError::Overflow)?;
            <Nonce<T>>::put(new_nonce);

            // update TradePairs
            TradePairs::insert(hash, tp.clone());
            <TradePairsHashByBaseQuote<T>>::insert((base, quote), hash);

            // update TradePairIndex
            let index = Self::trade_pair_index();
            <TradePairsHashByIndex<T>>::insert(index, hash);
            let new_index =  index.checked_add(1).ok_or(ArithmeticError::Overflow)?;
            <TradePairsIndex<T>>::put(new_index);

            Self::deposit_event(Event::TradePairCreated {
                owner: sender,
                hash,
                trade_pair: tp
            });

            Ok(())
        }

        fn do_create_limit_order(sender: T::AccountId, base: T::Hash, quote: T::Hash, otype: OrderType, price: T::Price, sell_amount: Balance<T>) -> DispatchResult {
            Self::ensure_bounds(price, sell_amount)?;
            let buy_amount = Self::ensure_counterparty_amount_bounds(otype, price, sell_amount)?;

            let tp_hash = Self::ensure_trade_pair(base, quote)?;

            /* check token của sender, giả sử cặp BUSD/BTC 
             => Nếu Buy  => sender phải có BUSD
             => Nếu Sell => sender phải có BTC
            */ 
            let op_token_hash;
            match otype {
                OrderType::Buy => op_token_hash = base,
                OrderType::Sell => op_token_hash = quote
            };

            /* Ở đây 1 giao dịch sẽ được hiểu là mình đang bán token này và mua token kia 
                Ví dụ: Cặp BUSD/BTC
                => Đặt lệnh BUY  => mình đang bán BUSD, và mua BTC
                => Đặt lệnh SELL => mình đang bán BTC, và mua BUSD
                
                Nên trong 1 Order luôn có sell_amount và buy_amount, trong đó:
                => sell_amount: là số lượng token mình đang bán 
                => buy_amount: là số lượng token mình đang mua
            */ 
            let mut order = Order::new(base, quote, sender.clone(), price, sell_amount, buy_amount, OrderOpt::Limit, otype);
            let hash = order.hash;

            // Check số dư và đóng băng số dư của sender
            pallet_tokens::Pallet::<T>::ensure_free_balance(sender.clone(), op_token_hash, sell_amount)?;
            pallet_tokens::Pallet::<T>::do_freeze(sender.clone(), op_token_hash, sell_amount)?;
            Orders::insert(hash, order.clone());

             // update_ nonce
             let nonce = Self::nonce();
             let new_nonce = nonce.checked_add(1).ok_or(ArithmeticError::Overflow)?;
             <Nonce<T>>::put(new_nonce);

             <OwnedTPOpenedOrders<T>>::add_order(sender.clone(), tp_hash, order.hash);

             let owned_index = Self::owned_orders_index(sender.clone());
            OwnedOrders::<T>::insert((sender.clone(), owned_index), hash);
            let new_owned_index = owned_index.checked_add(1).ok_or(ArithmeticError::Overflow)?;
            OwnedOrdersIndex::<T>::insert(sender.clone(), new_owned_index);

            let tp_owned_index = Self::trade_pair_owned_order_index(tp_hash);
            TradePairOwnedOrders::<T>::insert((tp_hash, tp_owned_index), hash);
            let new_tp_owned_index = tp_owned_index.checked_add(1).ok_or(ArithmeticError::Overflow)?;
            TradePairOwnedOrdersIndex::<T>::insert(tp_hash, new_tp_owned_index);

            // order match
            let filled = Self::order_match(tp_hash, &mut order)?;

            // add order to the market order list
            if !filled {
                <OrderList<T>>::append(tp_hash, price, hash, order.remained_sell_amount, order.remained_buy_amount, otype);
            } else {
                <OwnedTPOpenedOrders<T>>::remove_order(sender.clone(), tp_hash, order.hash);
                <OwnedTPClosedOrders<T>>::add_order(sender.clone(), tp_hash, order.hash);
            }

            Self::deposit_event(Event::OrderCreated{
                owner: sender.clone(),
                base_token: base,
                quote_token: quote,
                order_hash: hash,
                limit_order:  order.clone()}
            );

            Ok(())
        }

        fn do_create_market_order(sender: T::AccountId, base: T::Hash, quote: T::Hash, otype: OrderType, sell_amount: Balance<T>)-> DispatchResult {
            let tp_hash = Self::ensure_trade_pair(base, quote)?;
            let head = <OrderList<T>>::read_head(tp_hash);
            let item_price = Self::next_match_price(&head, !otype);
            if let Some(price) = item_price {
                if price != T::Price::min_value() || price != T::Price::max_value() {
                    if otype == OrderType::Buy {
                        let temp_price = Self::into_128(price.clone())?;
                        let temp: Balance<T> = Self::from_128(temp_price/T::PriceFactor::get())?;

                        let sell_amount = sell_amount * temp;
                        Self::do_create_limit_order(sender.clone(), base, quote, otype, price, sell_amount)?;
                    } else {
                        Self::do_create_limit_order(sender.clone(), base, quote, otype, price, sell_amount)?;
                    }
                    let index = Self::owned_orders_index(sender.clone()).checked_sub(1).ok_or(<Error<T>>::OverflowError)?;
                    let o_hash = Self::owned_orders((sender.clone(), index));
                    if let Some(hash) = o_hash{
                        let order = Self::orders(hash);
                        if let Some(mut o) = order {
                            if !o.is_finished() {
                                let remained_buy_amount = o.remained_buy_amount;
                                let remained_sell_amount = o.remained_sell_amount;
                                o.sell_amount -= remained_sell_amount;
                                o.buy_amount -= remained_buy_amount;
                                o.remained_buy_amount = Zero::zero();
                                o.remained_sell_amount = Zero::zero();
                                o.status = OrderStatus::Filled;
                                Orders::insert(hash, o);
                                if otype == OrderType::Buy {
                                    Self::do_create_market_order(sender.clone(), base, quote, otype, remained_buy_amount)?;
                                } else {
                                    Self::do_create_market_order(sender.clone(), base, quote, otype, remained_sell_amount)?;
                                }
                            }
                        }
                    }
                }
            }
            Ok(())
        }

        fn do_cancel_limit_order(sender: T::AccountId, order_hash: T::Hash) -> DispatchResult {
            let mut order = Self::orders(order_hash).ok_or(<Error<T>>::NoMatchingOrder)?;
    
            ensure!(order.owner == sender, <Error<T>>::CanOnlyCancelOwnOrder);
    
            ensure!(!order.is_finished(), <Error<T>>::CanOnlyCancelNotFinishedOrder);
    
            let tp_hash = Self::ensure_trade_pair(order.base, order.quote)?;
    
            <OrderList<T>>::remove_order(tp_hash, order.price, order.hash, order.sell_amount, order.buy_amount)?;
    
            order.status = OrderStatus::Canceled;
            <Orders<T>>::insert(order_hash, order.clone());
    
            <OwnedTPOpenedOrders<T>>::remove_order(sender.clone(), tp_hash, order_hash);
            <OwnedTPClosedOrders<T>>::add_order(sender.clone(), tp_hash, order_hash);
    
            let sell_hash = match order.otype {
                OrderType::Buy => order.base,
                OrderType::Sell => order.quote,
            };
    
            pallet_tokens::Pallet::<T>::do_unfreeze(sender.clone(), sell_hash, order.remained_sell_amount)?;
    
            Self::deposit_event(Event::OrderCanceled {
                owner: sender,
                order_hash
            });
    
            Ok(())
        }

        fn order_match(tp_hash: T::Hash, order: &mut Order<T>) -> Result<bool, DispatchError> {
            let mut head = <OrderList<T>>::read_head(tp_hash);
    
            let end_item_price;
            let otype = order.otype;
            let oprice = order.price;
    
            if otype == OrderType::Buy {
                end_item_price = Some(T::Price::min_value());
            } else {
                end_item_price = Some(T::Price::max_value());
            }
    
            let tp = Self::trade_pairs(tp_hash).ok_or(<Error<T>>::NoMatchingTradePair)?;
            let give: T::Hash; // token bán
            let have: T::Hash; // token mua
    
            match otype {
                OrderType::Buy => { // nếu lệnh mua thì là mình sẽ bán BUSD và thu về BTC
                    give = tp.base;
                    have = tp.quote;
                },
                OrderType::Sell => { // nếu lệnh bán thì là mình sẽ bán BTC và thu về BUSD
                    give = tp.quote;
                    have = tp.base;
                },
            };
    
            loop {
                if order.status == OrderStatus::Filled {
                    break;
                }
                
                // Ở đây lấy next_match_price của !otype => bởi vì lệnh bán mình phải so với các item_price ở list mua thì mới check khớp lệnh được và ngược lại
                let item_price = Self::next_match_price(&head, !otype);
    
                if item_price == end_item_price {
                    break;
                }
    
                let item_price = item_price.ok_or(<Error<T>>::OrderMatchGetPriceError)?;
    
                if !Self::price_matched(oprice, otype, item_price){
                    break
                }
    
                // Lúc này item_price này đã khớp với order_price => lấy Price_item tại mức giá này ra và check list order
                let item = <LinkedItemList<T>>::get((tp_hash, Some(item_price))).ok_or(<Error<T>>::OrderMatchGetLinkedListItemError)?;
                for ohash in item.orders.iter() {
    
                    let mut o = Self::orders(ohash).ok_or(<Error<T>>::OrderMatchGetOrderError)?;
    
                    let (base_qty, quote_qty) = Self::calculate_ex_amount(&o, &order)?;
    
                    let give_qty: Balance<T>;
                    let have_qty: Balance<T>;
                    match otype {
                        OrderType::Buy => { // nếu lệnh mua thì là mình sẽ bán BUSD và thu về BTC
                            give_qty = base_qty;
                            have_qty = quote_qty;
                        },
                        OrderType::Sell => { // nếu lệnh bán thì là mình sẽ bán BTC và thu về BUSD
                            give_qty = quote_qty;
                            have_qty = base_qty;
                        },
                    };
    
                    if order.remained_sell_amount == order.sell_amount {
                        order.status = OrderStatus::PartialFilled;
                    }
    
                    if o.remained_sell_amount == o.sell_amount {
                        o.status = OrderStatus::PartialFilled;
                    }
    
                    pallet_tokens::Pallet::<T>::do_unfreeze(order.owner.clone(), give, give_qty)?;
                    pallet_tokens::Pallet::<T>::do_unfreeze(o.owner.clone(), have, have_qty)?;
                    pallet_tokens::Pallet::<T>::do_transfer(order.owner.clone(), o.owner.clone(), give, give_qty)?;
                    pallet_tokens::Pallet::<T>::do_transfer(o.owner.clone(), order.owner.clone(), have, have_qty)?;
    
                    order.remained_sell_amount = order.remained_sell_amount.checked_sub(&give_qty).ok_or(<Error<T>>::OrderMatchSubstractError)?;
                    order.remained_buy_amount = order.remained_buy_amount.checked_sub(&have_qty).ok_or(<Error<T>>::OrderMatchSubstractError)?;
    
                    o.remained_sell_amount = o.remained_sell_amount.checked_sub(&have_qty).ok_or(<Error<T>>::OrderMatchSubstractError)?;
                    o.remained_buy_amount = o.remained_buy_amount.checked_sub(&give_qty).ok_or(<Error<T>>::OrderMatchSubstractError)?;
    
                    if order.remained_buy_amount == Zero::zero() {
                        order.status = OrderStatus::Filled;
                        if order.remained_sell_amount != Zero::zero() {
                            pallet_tokens::Pallet::<T>::do_unfreeze(order.owner.clone(), give, order.remained_sell_amount)?;
                            order.remained_sell_amount = Zero::zero();
                        }
    
                        <OwnedTPOpenedOrders<T>>::remove_order(order.owner.clone(), tp_hash, order.hash);
                        <OwnedTPClosedOrders<T>>::add_order(order.owner.clone(), tp_hash, order.hash);
    
                        ensure!(order.is_finished(), <Error<T>>::OrderMatchOrderIsNotFinished);
                    }
    
                    if o.remained_buy_amount == Zero::zero() {
                        o.status = OrderStatus::Filled;
                        if o.remained_sell_amount != Zero::zero() {
                            pallet_tokens::Pallet::<T>::do_unfreeze(o.owner.clone(), have, o.remained_sell_amount)?;
                            o.remained_sell_amount = Zero::zero();
                        }
    
                        <OwnedTPOpenedOrders<T>>::remove_order(o.owner.clone(), tp_hash, o.hash);
                        <OwnedTPClosedOrders<T>>::add_order(o.owner.clone(), tp_hash, o.hash);
    
                        ensure!(o.is_finished(), <Error<T>>::OrderMatchOrderIsNotFinished);
                    }
    
                    Orders::insert(order.hash.clone(), order.clone());
                    Orders::insert(o.hash.clone(), o.clone());
    
                    // save the trade pair market data
                    Self::set_tp_market_data(tp_hash, o.price, quote_qty)?;
    
                    // update maker order's amount in market
                    <OrderList<T>>::update_reduce_amount(tp_hash, o.price, have_qty, give_qty);
    
                    // remove the matched order
                    <OrderList<T>>::remove_order_match_price(tp_hash, !otype);
    
                    // save the trade data
                    let trade = Trade::new(tp.base, tp.quote, &o, &order, base_qty, quote_qty);
                    Trades::insert(trade.hash, trade.clone());
    
                    Self::deposit_event(Event::TradeCreated{
                        owner: order.owner.clone(),
                        base_token: tp.base,
                        quote_token: tp.quote,
                        trade_hash: trade.hash,
                        trade: trade.clone()}
                    );
    
                    // save trade reference data to store
                    <OrderOwnedTrades<T>>::add_trade(order.hash, trade.hash, None)?;
                    <OrderOwnedTrades<T>>::add_trade(o.hash, trade.hash, None)?;
    
                    <OwnedTrades<T>>::add_trade(order.owner.clone(), trade.hash,None)?;
                    <OwnedTrades<T>>::add_trade(o.owner.clone(), trade.hash, None)?;
    
                    <OwnedTPTrades<T>>::add_trade(order.owner.clone(), tp_hash, trade.hash)?;
                    <OwnedTPTrades<T>>::add_trade(o.owner.clone(), tp_hash, trade.hash)?;
    
                    <TradePairOwnedTrades<T>>::add_trade(tp_hash, trade.hash, None)?;
    
                    if order.status == OrderStatus::Filled {
                        break
                    }
                }
    
                head = <OrderList<T>>::read_head(tp_hash);
            }
    
            if order.status == OrderStatus::Filled {
                Ok(true)
            } else {
                Ok(false)
            }
        }
        
        // fn check bound
        fn ensure_bounds(price: T::Price, sell_amount: Balance<T>) -> DispatchResult{
            // check giá đặt phải > 0 và < max của type Price
            ensure!(price > Zero::zero() && price <= T::Price::max_value(), <Error<T>>::BoundsCheckFailedPrice);
            // check số lượng bán phải > 0 và < max của type Balance
            ensure!(sell_amount > Zero::zero() && sell_amount <= <Balance<T>>::max_value(), <Error<T>>::BoundsCheckFailedAmount);
            Ok(())
        }

        /* fn đảm bảo amount_base_token khớp với amount_quote_token để đáp ứng được 1 giao dịch
            ví dụ: 1 BTC = 20 BUSD hoặc ngược lại
            ở đây sẽ check tương ứng với mức giá đặt ra thì sẽ tương ứng với bao nhiêu
        */ 
        fn ensure_counterparty_amount_bounds(otype: OrderType, price: T::Price, amount: Balance<T>) -> Result<Balance<T>, Error<T>> {
            let price_u256 = U256::from(Self::into_128(price)?);
            let amount_u256 = U256::from(Self::into_128(amount)?);
            let max_balance_u256 = U256::from(Self::into_128(<Balance<T>>::max_value())?);
            let price_factor_u256 = U256::from(T::PriceFactor::get());

            let amount_v2: U256;
            let counterparty_amount: U256;

            match otype {
                OrderType::Buy => {
                    counterparty_amount = amount_u256 * price_factor_u256 / price_u256;
                    amount_v2 = counterparty_amount * price_u256 / price_factor_u256;
                },
                OrderType::Sell => {
                    counterparty_amount = amount_u256 * price_u256 / price_factor_u256;
                    amount_v2 = counterparty_amount * price_factor_u256 / price_u256;
                }
            }

            ensure!(amount_u256 == amount_v2, <Error<T>>::BoundsCheckFailedAmount2);
            ensure!(counterparty_amount != 0.into() && counterparty_amount <= max_balance_u256, <Error<T>>::BoundsCheckFailedAmountCounter);

            // change to u128
            let result: u128 = counterparty_amount.try_into().map_err(|_| <Error<T>>::OverflowError)?;
            Self::from_128(result)
        }

        // fn check cặp tp có tồn tại
        fn ensure_trade_pair(base: T::Hash, quote: T::Hash) -> Result<T::Hash, Error<T>> {
            let bq = Self::trade_pair_hash_by_base_quote((base, quote));
            ensure!(bq.is_some(), <Error<T>>::NoMatchingTradePair);

            match bq {
                Some(bq) => Ok(bq),
                None => Err(<Error<T>>::NoMatchingTradePair.into()),
            }
        }

        fn into_128<A: TryInto<u128>>(input: A) -> Result<u128, Error<T>> {
            TryInto::<u128>::try_into(input).map_err(|_| <Error<T>>::NumberCastError.into())
        }

        fn from_128<A: TryFrom<u128>>(input: u128) -> Result<A, Error<T>> {
            TryFrom::<u128>::try_from(input).map_err(|_| <Error<T>>::NumberCastError.into())
        }
    
        // fn này để tính toán lại số token_base và quote sau khi khớp lệnh giao dịch: maker là lệnh tạo trước, taker là lệnh tạo sau
        fn calculate_ex_amount(maker_order: &Order<T>, taker_order: &Order<T>) -> Result<(Balance<T>, Balance<T>), Error<T>> {
            let buyer_order;
            let seller_order;
            if taker_order.otype == OrderType::Buy {
                buyer_order = taker_order;
                seller_order = maker_order;
            } else {
                buyer_order = maker_order;
                seller_order = taker_order;
            }
    
            // todo: overflow checked need
            // todo: optimization need,
            let mut seller_order_filled = true;
            if seller_order.remained_buy_amount <= buyer_order.remained_sell_amount { // seller_order is Filled
                let quote_qty: u128 = Self::into_128(seller_order.remained_buy_amount)? * T::PriceFactor::get() / maker_order.price.into();
                if Self::into_128(buyer_order.remained_buy_amount)? < quote_qty {
                    seller_order_filled = false;
                }
            } else {
                let base_qty: u128 = Self::into_128(buyer_order.remained_buy_amount)? * maker_order.price.into() / T::PriceFactor::get();
                if Self::into_128(seller_order.remained_buy_amount)? >= base_qty {
                    seller_order_filled = false;
                }
            }
    
            // if seller_order.remained_buy_amount <= buyer_order.remained_sell_amount { // seller_order is Filled
            if seller_order_filled {
                let mut quote_qty: u128 = Self::into_128(seller_order.remained_buy_amount)? * T::PriceFactor::get() / maker_order.price.into();
                let buy_amount_v2 = quote_qty * Self::into_128(maker_order.price)? / T::PriceFactor::get();
                if buy_amount_v2 != Self::into_128(seller_order.remained_buy_amount)? && Self::into_128(buyer_order.remained_buy_amount)? > quote_qty // have fraction, seller(Filled) give more to align
                {
                    quote_qty = quote_qty + 1;
                }
    
                return Ok
                    ((
                        seller_order.remained_buy_amount,
                        Self::from_128(quote_qty)?
                    ))
            } else { // buyer_order is Filled
                let mut base_qty: u128 = Self::into_128(buyer_order.remained_buy_amount)? * maker_order.price.into() / T::PriceFactor::get();
                let buy_amount_v2 = base_qty * T::PriceFactor::get() / maker_order.price.into();
                if buy_amount_v2 != Self::into_128(buyer_order.remained_buy_amount)? && Self::into_128(seller_order.remained_buy_amount)? > base_qty // have fraction, buyer(Filled) give more to align
                {
                    base_qty = base_qty + 1;
                }
    
                return Ok
                    ((
                        Self::from_128(base_qty)?,
                        buyer_order.remained_buy_amount
                    ))
            }
        }

        fn next_match_price(item: &OrderItem<T>, otype: OrderType) -> Option<T::Price> {
            if otype == OrderType::Buy {
                item.prev
            } else {
                item.next
            }
        }
        
        /* fn lấy price_matched tiếp theo
            Trong fn order_match sẽ check giá trị trả về từ hàm này và lấy toán tử ! của nó
            Giả sử, có 1 lệnh mua tại giá 2$, xong bắt đầu có 1 lệnh bán tại giá 2$ 
            => order_price: 2$, otype: Sell, price_item_price: 2$
            => fn này sẽ trả về true => tiếp tục loop trong fn order_match

            Giả sử, có 1 lệnh mua tại giá 2$, xong bắt đầu có 1 lệnh bán tại giá 3$
            => order_price: 3$, otype: Sell, price_item_price: 2$
            => fn này sẽ trả về false => break loop trong fn order_match

            => Mục đích của hàm này là check giá khớp lệnh tiếp theo nếu khớp thì trả về true còn ko thì trả về false
         */
        fn price_matched(order_price: T::Price, otype: OrderType, price_item_price: T::Price) -> bool {
            match otype {
                OrderType::Sell => order_price <= price_item_price,
                OrderType::Buy => order_price >= price_item_price
            }
        }
    
        fn set_tp_market_data(tp_hash: T::Hash, price: T::Price, amount: Balance<T>) -> Result<(), Error<T>> {
            // get trade_pair nếu nó tồn tại
            let mut tp = Self::trade_pairs(tp_hash).ok_or(<Error<T>>::NoMatchingTradePair)?;

            // update match_price
            tp.latest_matched_price = Some(price);

            // get data_bucket và update thông tin
            let (mut sum_volume, mut highest_price, mut lowest_pricce) = <TPTradeDataBucket<T>>::get((tp_hash, frame_system::Pallet::<T>::block_number())).unwrap_or((Default::default(), None, None));
            sum_volume += amount;

            match highest_price {
                Some(tp_h_price) => {
                    if price > tp_h_price {
                        highest_price = Some(price);
                    }
                }, 
                None => {
                    highest_price = Some(price);
                }
            }

            match lowest_pricce {
                Some(tp_l_price) => {
                    if price < tp_l_price {
                        lowest_pricce = Some(price);
                    }
                }, 
                None => {
                    lowest_pricce = Some(price);
                }
            }

            <TPTradeDataBucket<T>>::insert((tp_hash, frame_system::Pallet::<T>::block_number()), (sum_volume, highest_price, lowest_pricce));
            <TradePairs<T>>::insert(tp_hash, tp);

            Ok(())
        }
    
    }
}