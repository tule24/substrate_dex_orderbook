use codec::{Encode, Decode, EncodeLike};
use frame_support::{
    sp_std::marker::PhantomData,
    sp_std::borrow::Borrow,
    sp_runtime::traits::{AtLeast32Bit, Bounded},
    sp_runtime::DispatchResult,
    pallet_prelude::Member, 
    StorageMap,RuntimeDebug, Parameter, ensure, 
};

pub use crate as trade;
type OrderType = trade::OrderType;

#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
pub struct LinkedItem<K1, K2, K3> {
    pub prev: Option<K2>,
    pub next: Option<K2>,
    pub price: Option<K2>,
    pub buy_amount: K3,
    pub sell_amount: K3,
    pub orders: Vec<K1> // remove the item at 0 index will caused performance issue, should be optimized
}

pub struct LinkedList<T, S, K1, K2, K3>(PhantomData<(T, S, K1, K2, K3)>);

impl<T, S, K1, K2, K3> LinkedList<T, S, K1, K2, K3>
where 
    T: trade::Config,
    S: StorageMap<(K1, Option<K2>), LinkedItem<K1, K2, K3>, Query = Option<LinkedItem<K1, K2, K3>>>,
    K1: EncodeLike + Encode + Decode + Clone + Borrow<<T as frame_system::Config>::Hash> + Copy + PartialEq + AsRef<[u8]>,
    K2: Parameter + Default + Member + AtLeast32Bit + Bounded + Copy,
    K3: Parameter + Default + Member + AtLeast32Bit + Bounded + Copy,
{
    pub fn read_head(key: K1) -> LinkedItem<K1, K2, K3> {
        Self::read(key, None)
    }

    #[allow(dead_code)]
    pub fn read_bottom(key: K1) -> LinkedItem<K1, K2, K3> {
        Self::read(key, Some(K2::min_value()))
    }

    #[allow(dead_code)]
    pub fn read_top(key: K1) -> LinkedItem<K1, K2, K3> {
        Self::read(key, Some(K2::max_value()))
    }

    pub fn read(key1: K1, key2: Option<K2>) -> LinkedItem<K1, K2, K3> {
        S::get((key1, key2)).unwrap_or_else(|| {
            let bottom = LinkedItem{
                prev: Some(K2::max_value()),
                next: None,
                price: Some(K2::min_value()),
                orders: Vec::<K1>::new(),
                buy_amount: Default::default(),
                sell_amount: Default::default()
            };

            let top = LinkedItem{
                prev: None,
                next: Some(K2::min_value()),
                price: Some(K2::max_value()),
                orders: Vec::<K1>::new(),
                buy_amount: Default::default(),
                sell_amount: Default::default()
            };

            let head = LinkedItem{
                prev: Some(K2::min_value()),
                next: Some(K2::max_value()),
                price: None,
                orders: Vec::<K1>::new(),
                buy_amount: Default::default(),
                sell_amount: Default::default()
            };

            Self::write(key1, bottom.price, bottom);
            Self::write(key1, top.price, top);
            Self::write(key1, head.price, head);

            head
        })
    }

    pub fn write(key1: K1, key2: Option<K2>, item: LinkedItem<K1, K2, K3>) {
        S::insert((key1, key2), item);
    }

    pub fn append(key1: K1, key2: K2, value: K1, sell_amount: K3, buy_amount: K3, order_type: OrderType) {
        let item = S::get((key1, Some(key2)));
        match item {
            Some(mut item) => {
                item.orders.push(value);
                item.buy_amount += buy_amount;
                item.sell_amount += sell_amount;
                Self::write(key1, Some(key2), item);
                return;
            },
            None => {
                let start_item;
                let end_item;

                match order_type{
                    OrderType::Buy => {
                        start_item = Some(K2::min_value());
                        end_item = None;
                    },
                    OrderType::Sell => {
                        start_item = None;
                        end_item = Some(K2::max_value());
                    }
                }

                let mut item = Self::read(key1, start_item);

                while item.next != end_item {
                    match item.next {
                        None => {},
                        Some(price) => {
                            if key2 < price {
                                break;
                            }
                        }
                    }
                    item = Self::read(key1, item.next);
                }

                // update new_prev
                let new_prev = LinkedItem {
                    next: Some(key2),
                    ..item
                };
                Self::write(key1, new_prev.price, new_prev.clone());

                // update new_next
                let next = Self::read(key1, item.next);
                let new_next = LinkedItem{
                    prev: Some(key2),
                    ..next
                };
                Self::write(key1, new_next.price, new_next.clone());

                // update key2
                let mut v = Vec::new();
                v.push(value);
                let item = LinkedItem{
                    prev: new_prev.price,
                    next: new_next.price,
                    buy_amount,
                    sell_amount,
                    orders: v,
                    price: Some(key2)
                };
                Self::write(key1, Some(key2), item);
            }
        };
    }

    pub fn next_match_price(item: &LinkedItem<K1, K2, K3>, order_type: OrderType) -> Option<K2> {
        if order_type == OrderType::Buy {
            item.prev
        } else {
            item.next
        }
    }

    pub fn update_amount(key1: K1, key2: K2, sell_amount: K3, buy_amount: K3) {
        let mut item = Self::read(key1, Some(key2));
        item.buy_amount -= buy_amount;
        item.sell_amount -= sell_amount;
        Self::write(key1, Some(key2), item);
    }

    pub fn remove_all(key1: K1, order_type: OrderType) {
        let end_item;

        if order_type == OrderType::Buy {
            end_item = Some(K2::min_value());
        } else {
            end_item = Some(K2::max_value());
        }

        let mut head = Self::read_head(key1);

        loop {
            let key2 = Self::next_match_price(&head, order_type);
            if key2 == end_item {
                break;
            }
            match Self::remove_orders_in_one_item(key1, key2.unwrap()) {
                Err(_) => break,
                _ => {}
            };

            head = Self::read_head(key1);
        }
    }

    pub fn remove_order(key1: K1, key2: K2, order_hash: K1, sell_amount: K3, buy_amount: K3) -> DispatchResult {
        match S::get((key1, Some(key2))) {
            Some(mut item) => {
                ensure!(item.orders.contains(&order_hash), "Cancel the order but not in market order list");
                item.orders.retain(|&x| x != order_hash); // like filter
                item.buy_amount -= buy_amount;
                item.sell_amount -= sell_amount;
                Self::write(key1, Some(key2), item.clone());

                if item.orders.len() == 0 {
                    Self::remove_item(key1, key2);
                }
            },
            None => {}
        }
        Ok(())   
    }

    pub fn remove_item(key1: K1, key2: K2) {
        if let Some(item) = S::take((key1, Some(key2))) {
            S::mutate((key1.clone(), item.prev), |x| {
                if let Some(x) = x {
                    x.next = item.next;
                }
            });

            S::mutate((key1.clone(), item.next), |x| {
                if let Some(x) = x {
                    x.prev = item.prev;
                }
            })
        }
    }

     // when the order is canceled, it should be remove from Sell / Buy orders
     pub fn remove_orders_in_one_item(key1: K1, key2: K2) -> DispatchResult {
        match S::get((key1, Some(key2))) {
            Some(mut item) => {
                while item.orders.len() > 0 {
                    let order_hash = item.orders.get(0).ok_or("Cannot get order hash")?;

                    let order = <trade::Config<T>>::order(order_hash.borrow()).ok_or("Cannot get order");
                    ensure!(order.is_finished(), "Try to remove not finished order");
                    item.orders.remove(0);
                    Self::write(key1, Some(key2), item.clone());
                }
                
                if item.orders.len() == 0 {
                    Self::remove_item(key1, key2);
                }
            },
             None => {}
        }

        Ok(())
     }
}

