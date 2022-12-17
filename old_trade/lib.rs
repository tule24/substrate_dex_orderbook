#![cfg_attr(not(feature = "std"), no_std)]

mod linked_price_list;
mod lib_new;
pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
    use frame_support::{
        pallet_prelude::*,
        sp_runtime::{
            traits::{Hash, Bounded, AtLeast32BitUnsigned, Zero},
            ArithmeticError
        }, 
        sp_std::ops::Not,
        traits::{Randomness, Currency},
        Parameter, Blake2_128Concat,
    };
	use frame_system::pallet_prelude::*;
    use sp_core::U256;
    use crate::linked_price_list::{PriceItem, PriceList};

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + pallet_tokens::Config{
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        type Price: Parameter + Default + Member + Bounded + AtLeast32BitUnsigned + Copy + From<u128> + Into<u128>;
        type PriceFactor: Get<u128>;
        type BlocksPerDay: Get<u32>;
        type OpenedOrdersArrayCap: Get<u8>;
        type ClosedOrdersArrayCap: Get<u8>;    
        type Currency: Currency<Self::AccountId>;
        type TradeRandom: Randomness<Self::Hash, Self::BlockNumber>;
	}

    type Balance<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
    
    #[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
    /* struct TradePair để quản lý các cặp trade_pair
        Ví dụ: cặp BTC/BUSD
        => Trong đó, base_token là BTC, và quote_token là BUSD 
    */ 
    pub struct TradePair<T: Config> {
        // token info
        hash: T::Hash,                                 // tp_hash
        base: T::Hash,                                 // base_token
        quote: T::Hash,                                // quote_token               

        latest_matched_price: Option<T::Price>,        // giá khớp lệnh gần nhất

        one_day_trade_volume: Balance<T>,              // tổng khối lượng 24h
        one_day_highest_price: Option<T::Price>,       // giá cao nhất 24h
        one_day_lowest_price: Option<T::Price>,        // giá thấp nhất 24j 
    } 

    #[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
    pub enum OrderType {
        Buy,
        Sell
    }
    impl Not for OrderType {
        type Output = Self;

        fn not(self) -> Self::Output {
            match self {
                OrderType::Buy => OrderType::Sell,
                OrderType::Sell => OrderType::Buy 
            }
        }
    }

    #[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
    pub enum OrderStatus {
        Created,                // Khởi tạo
        PartialFilled,          // Khớp 1 phần
        Filled,                 // Khớp toàn bộ
        Canceled                // Hủy
    }

    #[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
    /* struct LimitOrder quản lý các lệnh limit */
    pub struct LimitOrder<T: Config> {
        pub hash: T::Hash,                      // order_hash
        pub base: T::Hash,                      // base_token
        pub quote: T::Hash,                     // quote_token

        pub owner: T::AccountId,                // sender
        pub price: T::Price,                    // price
        pub sell_amount: Balance<T>,            // số lượng mua
        pub buy_amount: Balance<T>,             // số lượng bán
        pub remained_sell_amount: Balance<T>,   // số lượng mua còn lại
        pub remained_buy_amount: Balance<T>,    // số lượng bán còn lại
        
        pub otype: OrderType,                   // Buy / Sell
        pub status: OrderStatus                 // Trạng thái Order
    }

    impl<T: Config> LimitOrder<T> {
        fn new(base: T::Hash, quote: T::Hash, owner: T::AccountId, price: T::Price, sell_amount: Balance<T>, buy_amount: Balance<T>, otype: OrderType) -> Self {
            let nonce = <Nonce<T>>::get();
            let random = T::TradeRandom::random_seed().0;
            let hash = (random, base, quote, owner.clone(), price, sell_amount, buy_amount, otype.clone(), nonce, frame_system::Pallet::<T>::block_number()).using_encoded(T::Hashing::hash);

            LimitOrder { hash, base, quote, owner, price, sell_amount, buy_amount, remained_sell_amount: sell_amount, remained_buy_amount: buy_amount, otype, status:OrderStatus::Created, }
        }

        fn is_finished(&self) -> bool {
            self.remained_buy_amount == Zero::zero() && self.status == OrderStatus::Filled ||
            self.status == OrderStatus::Canceled
        }
    }

    #[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
    // strutc Trade => quản lý các giao dịch
    pub struct Trade<T: Config> {
        hash: T::Hash,
        base: T::Hash,
        quote: T::Hash,

        buyer: T::AccountId,                  // have base
        seller: T::AccountId,                 // have quote
        maker: T::AccountId,                  // order được tạo trước
        taker: T::AccountId,                  // order match với maker ở trên
        otype: OrderType,                     // taker order's type
        price: T::Price,                      // maker order's price
        base_amount: Balance<T>,              // base token amount to exchange
        quote_amount: Balance<T>              // quote token amount to exchange
    }

    impl<T: Config> Trade<T> {
        fn new(base: T::Hash, quote: T::Hash, maker_order: &LimitOrder<T>, taker_order: &LimitOrder<T>, base_amount: Balance<T>, quote_amount: Balance<T>) -> Self {
            let nonce = <Nonce<T>>::get();
            let random = T::TradeRandom::random_seed().0;
            let hash = (random, frame_system::Pallet::<T>::block_number(), nonce, maker_order.hash, maker_order.remained_sell_amount, maker_order.owner.clone(), taker_order.hash, taker_order.remained_sell_amount, taker_order.owner.clone()).using_encoded(T::Hashing::hash);

            let _new_nonce = nonce.checked_add(1);
            if let Some(new_nonce) = _new_nonce {
                <Nonce<T>>::put(new_nonce);
            }

            // check thằng nào bán, thằng nào mua => otype 2 thằng này phải khác nhau
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

    type OrderLinkedItem<T> = PriceItem<<T as frame_system::Config>::Hash, <T as Config>::Price, Balance<T>>;
    type OrderLinkedItemList<T> = PriceList<T, LinkedItemList<T>, <T as frame_system::Config>::Hash, <T as Config>::Price, Balance<T>>;

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
    #[pallet::getter(fn linked_item)]
    pub type LinkedItemList<T: Config> = StorageMap<_, Blake2_128Concat, (T::Hash, Option<T::Price>), OrderLinkedItem<T>>; 

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

    #[pallet::storage]
	#[pallet::getter(fn nonce)]
	pub type Nonce<T: Config> = StorageValue<_, u64, ValueQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
        // emit khi 1 tradepair được tạo => (người tạo, tp_hash, tp)
        TradePairCreated {
            owner: T::AccountId,            
            hash: T::Hash,                  
            tradePair: TradePair<T>            
        },
        // emit khi order được tạo => (người tạo, base_token, quote_token, order_hash, limit_order)
        OrderCreated {
            owner: T::AccountId,
            baseToken: T::Hash,
            quoteToken: T::Hash,
            orderHash: T::Hash,
            limitOrder: LimitOrder<T>
        },
        // emit khi trade được tạo => (người tạo, base_token, quote_token, trade_hash, trade)
        TradeCreated {
            owner: T::AccountId,
            baseToken: T::Hash,
            quoteToken: T::Hash,
            tradeHash: T::Hash,
            trade: Trade<T>
        },
        // emit khi order cancel => (người cancel, order_hash)
        OrderCanceled {
            owner: T::AccountId,
            orderHash: T::Hash
        }
    }

    impl<T: Config> OrderOwnedTrades<T> {
        fn add_trade(order_hash: T::Hash, trade_hash: T::Hash) {
            let index = <OrderOwnedTradesIndex<T>>::get(&order_hash);
            Self::insert((order_hash.clone(), index), trade_hash);
            <OrderOwnedTradesIndex<T>>::insert(order_hash, index + 1);
        }
    }

    impl<T: Config> OwnedTrades<T> {
        fn add_trade(account_id: T::AccountId, trade_hash: T::Hash) {
            let index = <OwnedTradesIndex<T>>::get(&account_id);
            Self::insert((account_id.clone(), index), trade_hash);
            <OwnedTradesIndex<T>>::insert(account_id, index + 1);
        }
    }

    impl<T: Config> TradePairOwnedTrades<T> {
        fn add_trade(tp_hash: T::Hash, trade_hash: T::Hash) {
            let index = <TradePairOwnedTradesIndex<T>>::get(&tp_hash);
            Self::insert((tp_hash.clone(), index), trade_hash);
            <TradePairOwnedTradesIndex<T>>::insert(tp_hash, index + 1);
        }
    }

    impl<T: Config> OwnedTPTrades<T> {
        fn add_trade(account_id: T::AccountId, tp_hash: T::Hash, trade_hash: T::Hash) {
            let index = <OwnedTPTradesIndex<T>>::get((account_id.clone(), tp_hash));
            Self::insert((account_id.clone(), tp_hash, index), trade_hash);
            <OwnedTPTradesIndex<T>>::insert((account_id.clone(), tp_hash), index + 1);
        }
    }

    impl<T: Config> OwnedTPOpenedOrders<T> {
        fn add_order(account_id: T::AccountId, tp_hash: T::Hash, order_hash: T::Hash) {
    
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
                    if orders.len() == T::OpenedOrdersArrayCap::get() as usize {
                        orders.pop();
                    }
    
                    <OwnedTPOpenedOrders<T>>::insert((account_id, tp_hash), orders);
                }
            }
        }
    
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

    impl<T: Config> OwnedTPClosedOrders<T> {
        fn add_order(account_id: T::AccountId, tp_hash: T::Hash, order_hash: T::Hash) {
    
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

	#[pallet::error]
	pub enum Error<T> {
       /// Price bounds check failed
		BoundsCheckFailed,
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
    }

	#[pallet::call]
	impl<T: Config> Pallet<T> {
        #[pallet::weight(100_000_000)]
        pub fn create_trade_pair(
            origin: OriginFor<T>,
            base: T::Hash,
            quote: T::Hash
        ) -> DispatchResult {
            let sender = ensure_signed(origin)?;
            Self::do_create_trade_pair(sender, base, quote)
        }

        #[pallet::weight(100_000_000)]
        pub fn create_limit_order(
            origin: OriginFor<T>,
            base: T::Hash,
            quote: T::Hash,
            otype: OrderType,
            price: T::Price,
            sell_amount: Balance<T>
        ) -> DispatchResult {
            let sender = ensure_signed(origin)?;
            Self::do_create_limit_order(sender, base, quote, otype, price, sell_amount)
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

        #[pallet::weight(100_000_000)]
        pub fn cancel_limit_order(
            origin: OriginFor<T>,
            order_hash: T::Hash
        ) -> DispatchResult {
            let sender = ensure_signed(origin)?;
            Self::do_cancel_limit_order(sender, order_hash)
        }
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(block_number: T::BlockNumber) -> Weight {
			let days: T::BlockNumber = <<T as frame_system::Config>::BlockNumber as From<_>>::from(T::BlocksPerDay::get());

			if block_number <= days {
				return 1000
			}

			for index in 0 .. TradePairsIndex::get() {
				let tp_hash = TradePairsHashByIndex::<T>::get(index).unwrap();
				let mut tp = TradePairs::<T>::get(tp_hash).unwrap();
				let (amount, _, _) = TPTradeDataBucket::<T>::get((tp_hash, block_number - days));
				tp.one_day_trade_volume = tp.one_day_trade_volume - amount;
				TradePairs::<T>::insert(tp_hash, tp);

				let mut bucket = TPTradePriceBucket::<T>::get(tp_hash);
				if bucket.0.len() > 0 {
					bucket.0.remove(0);
				}
				if bucket.1.len() > 0 {
					bucket.1.remove(0);
				}
				TPTradePriceBucket::<T>::insert(tp_hash, bucket);
			}

			500_000
		}

        fn on_finalize(block_number: T::BlockNumber) {
			for index in 0 .. TradePairsIndex::get() {
				let tp_hash = TradePairsHashByIndex::<T>::get(index).unwrap();
				let mut tp = TradePairs::<T>::get(tp_hash).unwrap();

				let data_bucket = TPTradeDataBucket::<T>::get((tp_hash, block_number));
				
				let mut price_bucket = TPTradePriceBucket::<T>::get(tp_hash);
				price_bucket.0.push(data_bucket.1);
				price_bucket.1.push(data_bucket.2);
				TPTradePriceBucket::<T>::insert(tp_hash, &price_bucket);

				let mut h_price = T::Price::min_value();
				for price in price_bucket.0.iter() {
					if let &Some(price) = price {
						if price > h_price {
							h_price = price;
						}
					}
				}

				let mut l_price = T::Price::max_value();
				for price in price_bucket.1.iter() {
					if let &Some(price) = price {
						if price < l_price {
							l_price = price;
						}
					}
				}

				tp.one_day_trade_volume = tp.one_day_trade_volume + data_bucket.0;
				
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
				
				TradePairs::<T>::insert(tp_hash, tp);
			}
		}
    }

	impl<T: Config> Pallet<T> {
        fn ensure_bounds(price: T::Price, sell_amount: Balance<T>) -> DispatchResult {
            ensure!(price > Zero::zero() && price <= T::Price::max_value(), <Error<T>>::BoundsCheckFailed);
            ensure!(sell_amount > Zero::zero() && sell_amount <= T::Balance::max_value(), <Error<T>>::BoundsCheckFailed);
            Ok(())
        }

        fn ensure_counterparty_amount_bounds(otype: OrderType, price:T::Price, amount: Balance<T>) -> Result<Balance<T>, Error<T>> {

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
                },
            }

            ensure!(amount_u256 == amount_v2, <Error<T>>::BoundsCheckFailed);
            ensure!(counterparty_amount != 0.into() && counterparty_amount <= max_balance_u256, <Error<T>>::BoundsCheckFailed);

            // todo: change to u128
            let result: u128 = counterparty_amount.try_into().map_err(|_| <Error<T>>::OverflowError)?;

            Self::from_128(result)
        }

        fn ensure_trade_pair(base: T::Hash, quote: T::Hash) -> Result<T::Hash, Error<T>> {
            let bq = Self::trade_pair_hash_by_base_quote((base, quote));
            ensure!(bq.is_some(), <Error<T>>::NoMatchingTradePair);
    
            match bq {
                Some(bq) => Ok(bq),
                None => Err(<Error<T>>::NoMatchingTradePair.into()),
            }
        }

        fn do_create_trade_pair(sender: T::AccountId, base: T::Hash, quote: T::Hash) -> DispatchResult {

            ensure!(base != quote, <Error<T>>::BaseEqualQuote);
    
            let base_owner = pallet_tokens::Pallet::<T>::owners(base);
            let quote_owner = pallet_tokens::Pallet::<T>::owners(quote);
    
            ensure!(base_owner.is_some() && quote_owner.is_some(), <Error<T>>::TokenOwnerNotFound);
    
            let base_owner = base_owner.unwrap();
            let quote_owner = quote_owner.unwrap();
    
            ensure!(sender == base_owner || sender == quote_owner, <Error<T>>::SenderNotEqualToBaseOrQuoteOwner);
    
            let bq = Self::trade_pair_hash_by_base_quote((base, quote));
            let qb = Self::trade_pair_hash_by_base_quote((quote, base));
    
            ensure!(!bq.is_some() && !qb.is_some(), <Error<T>>::TradePairExisted);
    
            let nonce = Nonce::get();
    
            let random = T::TradeRandom::random_seed().0;
            let hash = (random, frame_system::Pallet::<T>::block_number(), sender.clone(), base, quote, nonce).using_encoded(T::Hashing::hash);

            let tp = TradePair {
                hash, base, quote,
                latest_matched_price: None,
                one_day_trade_volume: Default::default(),
                one_day_highest_price: None,
                one_day_lowest_price: None,
            };
    
            let new_nonce = nonce.checked_add(1).ok_or(ArithmeticError::Overflow)?;
            <Nonce<T>>::put(new_nonce);

            TradePairs::insert(hash, tp.clone());
            <TradePairsHashByBaseQuote<T>>::insert((base, quote), hash);
    
            let index = Self::trade_pair_index();
            <TradePairsHashByIndex<T>>::insert(index, hash);
            let new_index = index.checked_add(1).ok_or(ArithmeticError::Overflow)?;
            <TradePairsIndex<T>>::put(new_index);
    
            Self::deposit_event(Event::TradePairCreated{
                owner: sender,
                hash,
                tradePair: tp
            });
    
            Ok(())
        }

        fn do_create_limit_order(sender: T::AccountId, base: T::Hash, quote: T::Hash, otype: OrderType, price: T::Price, sell_amount: Balance<T>) -> DispatchResult {

            Self::ensure_bounds(price, sell_amount)?;
            let buy_amount = Self::ensure_counterparty_amount_bounds(otype, price, sell_amount)?;

            let tp_hash = Self::ensure_trade_pair(base, quote)?;

            let op_token_hash;
            match otype {
                OrderType::Buy => op_token_hash = base,
                OrderType::Sell => op_token_hash = quote,
            };

            let mut order = LimitOrder::new(base, quote, sender.clone(), price, sell_amount, buy_amount, otype);
            let hash = order.hash;

            pallet_tokens::Pallet::<T>::ensure_free_balance(sender.clone(), op_token_hash, sell_amount)?;
            pallet_tokens::Pallet::<T>::do_freeze(sender.clone(), op_token_hash, sell_amount)?;
            Orders::insert(hash, order.clone());

            let nonce = Nonce::get();
            let new_nonce = nonce.checked_add(1).ok_or(ArithmeticError::Overflow)?;
            <Nonce<T>>::put(new_nonce);

            Self::deposit_event(Event::OrderCreated{
                owner: sender.clone(),
                baseToken: base,
                quoteToken: quote,
                orderHash: hash,
                limitOrder:  order.clone()}
            );
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
                <OrderLinkedItemList<T>>::append(tp_hash, price, hash, order.remained_sell_amount, order.remained_buy_amount, otype);
            } else {
                <OwnedTPOpenedOrders<T>>::remove_order(sender.clone(), tp_hash, order.hash);
                <OwnedTPClosedOrders<T>>::add_order(sender.clone(), tp_hash, order.hash);
            }

            Ok(())
        }

        fn order_match(tp_hash: T::Hash, order: &mut LimitOrder<T>) -> Result<bool, Error<T>> {
            let mut head = <OrderLinkedItemList<T>>::read_head(tp_hash);
    
            let end_item_price;
            let otype = order.otype;
            let oprice = order.price;
    
            if otype == OrderType::Buy {
                end_item_price = Some(T::Price::min_value());
            } else {
                end_item_price = Some(T::Price::max_value());
            }
    
            let tp = Self::trade_pair(tp_hash).ok_or(<Error<T>>::NoMatchingTradePair)?;
            let give: T::Hash;
            let have: T::Hash;
    
            match otype {
                OrderType::Buy => {
                    give = tp.base;
                    have = tp.quote;
                },
                OrderType::Sell => {
                    give = tp.quote;
                    have = tp.base;
                },
            };
    
            loop {
                if order.status == OrderStatus::Filled {
                    break;
                }
    
                let item_price = Self::next_match_price(&head, !otype);
    
                if item_price == end_item_price {
                    break;
                }
    
                let item_price = item_price.ok_or(<Error<T>>::OrderMatchGetPriceError)?;
    
                if !Self::price_matched(oprice, otype, item_price) {
                    break
                }
    
                let item = <LinkedItemList<T>>::get((tp_hash, Some(item_price))).ok_or(<Error<T>>::OrderMatchGetLinkedListItemError)?;
                for o in item.orders.iter() {
    
                    let mut o = Self::order(o).ok_or(<Error<T>>::OrderMatchGetOrderError)?;
    
                    let (base_qty, quote_qty) = Self::calculate_ex_amount(&o, &order)?;
    
                    let give_qty: T::Balance;
                    let have_qty: T::Balance;
                    match otype {
                        OrderType::Buy => {
                            give_qty = base_qty;
                            have_qty = quote_qty;
                        },
                        OrderType::Sell => {
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
    
                    pallet_tokens::Pallet::<T>::do_transfer(order.owner.clone(), give, o.owner.clone(), give_qty, None)?;
                    pallet_tokens::Pallet::<T>::do_transfer(o.owner.clone(), have, order.owner.clone(), have_qty, None)?;
    
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
                    <OrderLinkedItemList<T>>::update_amount(tp_hash, o.price, have_qty, give_qty);
    
                    // remove the matched order
                    <OrderLinkedItemList<T>>::remove_all(tp_hash, !otype);
    
                    // save the trade data
                    let trade = Trade::new(tp.base, tp.quote, &o, &order, base_qty, quote_qty);
                    Trades::insert(trade.hash, trade.clone());
    
                    Self::deposit_event(Event::TradeCreated{
                        owner: order.owner.clone(),
                        baseToken: tp.base,
                        quoteToken: tp.quote,
                        tradeHash: trade.hash,
                        trade: trade.clone()}
                    );
    
                    // save trade reference data to store
                    <OrderOwnedTrades<T>>::add_trade(order.hash, trade.hash);
                    <OrderOwnedTrades<T>>::add_trade(o.hash, trade.hash);
    
                    <OwnedTrades<T>>::add_trade(order.owner.clone(), trade.hash);
                    <OwnedTrades<T>>::add_trade(o.owner.clone(), trade.hash);
    
                    <OwnedTPTrades<T>>::add_trade(order.owner.clone(), tp_hash, trade.hash);
                    <OwnedTPTrades<T>>::add_trade(o.owner.clone(), tp_hash, trade.hash);
    
                    <TradePairOwnedTrades<T>>::add_trade(tp_hash, trade.hash);
    
                    if order.status == OrderStatus::Filled {
                        break
                    }
                }
    
                head = <OrderLinkedItemList<T>>::read_head(tp_hash);
            }
    
            if order.status == OrderStatus::Filled {
                Ok(true)
            } else {
                Ok(false)
            }
        }

        fn into_128<A: TryInto<u128>>(i: A) -> Result<u128, Error<T>> {
            TryInto::<u128>::try_into(i).map_err(|_| <Error<T>>::NumberCastError.into())
        }
    
        fn from_128<A: TryFrom<u128>>(i: u128) -> Result<A, Error<T>> {
            TryFrom::<u128>::try_from(i).map_err(|_| <Error<T>>::NumberCastError.into())
        }

        fn calculate_ex_amount(maker_order: &LimitOrder<T>, taker_order: &LimitOrder<T>) -> Result<(Balance<T>, Balance<T>), Error<T>> {
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
                let quote_qty: u128 =
                    Self::into_128(seller_order.remained_buy_amount)? * T::PriceFactor::get() / maker_order.price.into();
                if Self::into_128(buyer_order.remained_buy_amount)? < quote_qty {
                    seller_order_filled = false;
                }
            } else {
                let base_qty: u128 =
                    Self::into_128(buyer_order.remained_buy_amount)? * maker_order.price.into() / T::PriceFactor::get();
                if Self::into_128(seller_order.remained_buy_amount)? >= base_qty {
                    seller_order_filled = false;
                }
            }
    
            // if seller_order.remained_buy_amount <= buyer_order.remained_sell_amount { // seller_order is Filled
            if seller_order_filled {
                let mut quote_qty: u128 =
                    Self::into_128(seller_order.remained_buy_amount)? * T::PriceFactor::get() / maker_order.price.into();
                let buy_amount_v2 = quote_qty * Self::into_128(maker_order.price)? / T::PriceFactor::get();
                if buy_amount_v2 != Self::into_128(seller_order.remained_buy_amount)? &&
                    Self::into_128(buyer_order.remained_buy_amount)? > quote_qty // have fraction, seller(Filled) give more to align
                {
                    quote_qty = quote_qty + 1;
                }
    
                return Ok
                    ((
                        seller_order.remained_buy_amount,
                        Self::from_128(quote_qty)?
                    ))
            } else { // buyer_order is Filled
                let mut base_qty: u128 =
                    Self::into_128(buyer_order.remained_buy_amount)? * maker_order.price.into() / T::PriceFactor::get();
                let buy_amount_v2 = base_qty * T::PriceFactor::get() / maker_order.price.into();
                if buy_amount_v2 != Self::into_128(buyer_order.remained_buy_amount)? &&
                    Self::into_128(seller_order.remained_buy_amount)? > base_qty // have fraction, buyer(Filled) give more to align
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

        fn next_match_price(item: &OrderLinkedItem<T>, otype: OrderType) -> Option<T::Price> {
            if otype == OrderType::Buy {
                item.prev
            } else {
                item.next
            }
        }
    
        fn price_matched(order_price: T::Price, order_type: OrderType, linked_item_price: T::Price) -> bool {
            match order_type {
                OrderType::Sell => order_price <= linked_item_price,
                OrderType::Buy => order_price >= linked_item_price,
            }
        }

        pub fn set_tp_market_data(tp_hash: T::Hash, price: T::Price, amount: Balance<T>) -> DispatchResult {

            let mut tp = <TradePairs<T>>::get(tp_hash).ok_or(<Error<T>>::NoMatchingTradePair)?;
    
            tp.latest_matched_price = Some(price);
    
            let mut bucket = <TPTradeDataBucket<T>>::get((tp_hash, frame_system::Pallet::<T>::block_number()));
            bucket.0 = bucket.0 + amount;
    
            match bucket.1 {
                Some(tp_h_price) => {
                    if price > tp_h_price {
                        bucket.1 = Some(price);
                    }
                },
                None => {
                    bucket.1 = Some(price);
                },
            }
    
            match bucket.2 {
                Some(tp_l_price) => {
                    if price < tp_l_price {
                        bucket.2 = Some(price);
                    }
                },
                None => {
                    bucket.2 = Some(price);
                },
            }
    
            <TPTradeDataBucket<T>>::insert((tp_hash, frame_system::Pallet::<T>::block_number()), bucket);
            <TradePairs<T>>::insert(tp_hash, tp);
    
            Ok(())
        }

        fn do_cancel_limit_order(sender: T::AccountId, order_hash: T::Hash) -> DispatchResult {
            let mut order = Self::order(order_hash).ok_or(<Error<T>>::NoMatchingOrder)?;
    
            ensure!(order.owner == sender, <Error<T>>::CanOnlyCancelOwnOrder);
    
            ensure!(!order.is_finished(), <Error<T>>::CanOnlyCancelNotFinishedOrder);
    
            let tp_hash = Self::ensure_trade_pair(order.base, order.quote)?;
    
            <OrderLinkedItemList<T>>::remove_order(tp_hash, order.price, order.hash, order.sell_amount, order.buy_amount)?;
    
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
                orderHash: order_hash
            });
    
            Ok(())
        }
    }
}

