use codec::{Encode, Decode, EncodeLike};
use frame_support::{
    pallet_prelude::*,
    sp_runtime::{
        traits::{AtLeast32Bit, Bounded}
    },
    sp_std::{
        marker::PhantomData,
        borrow::Borrow
    },
    inherent::Vec,
    StorageDoubleMap, RuntimeDebug, Parameter, 
};
use scale_info::TypeInfo;
pub use crate as trade;
type OrderType = trade::OrderType;


#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
/* Ở đây dùng 3 generic type là P1, P2, P3.
 - P1: là order_hash hoặc token_hash => kiểu hash
 - P2: là mức giá => kiểu price
 - P3: là amount => kiểu balance
 */

// struct PriceItem => nó là các node trong linked_list chứa thông tin của 1 mức giá cụ thể
pub struct PriceItem<P1, P2, P3>{
    pub prev: Option<P2>,       // mức giá của item prev
    pub next: Option<P2>,       // mức giá của item next

    // price_item info
    pub price: Option<P2>,      // mức giá hiện tại của item
    pub buy_amount: P3,         // tổng lượng mua
    pub sell_amount: P3,        // tổng lượng bán
    pub orders: Vec<P1>         // list các order_hash tại mức giá của item
}

// struct PriceList => chính là linked_list, mỗi token sẽ có 1 linked_list để quản lý list giá
pub struct PriceList<T, S, P1, P2, P3>(PhantomData<(T, S, P1, P2, P3)>);
 
impl<T, S, P1, P2, P3> PriceList<T, S, P1, P2, P3>
where
    // config của mod trade
    T: trade::Config,
    // Mapping token_hash => mức giá => price_item tại mức giá đó
    S: StorageDoubleMap<P1, Option<P2>, PriceItem<P1, P2, P3>, Query = Option<PriceItem<P1, P2, P3>>>,
    // P1, P2, P3 tương tự như struct PriceItem
    P1: EncodeLike + Encode + Decode + Clone + Copy + PartialEq + Borrow<<T as frame_system::Config>::Hash>,
    P2: Parameter + Default + AtLeast32Bit + Bounded + Copy + EncodeLike + Encode + Decode,
    P3: Parameter + Default + AtLeast32Bit + Bounded + Copy
{
    /* fn new => init một Circular Linked List cho mỗi token, cấu trúc sẽ là: 
        + bottom: PriceItem tại min_value của kiểu dữ liệu P2 => tương ứng là node_head trong linked_list
        + head: PriceItem tại none => nó sẽ nằm giữa bottom và heap
        + top: PriceItem tại min_value của kiểu dữ liệu P2 => tương ứng là node_head trong linked_list

        Mô hình nó sẽ như sau:
        - Khi khởi tạo price_list sẽ tạo ra 3 node cố định là bottom, head và top

             ______________          Buy                  __________________              Sell               _________________
price_list:  |  Bottom    |<----------------------------->|     Head       |<------------------------------->|      Top      |
             |____________|                               |________________|                                 |_______________|
                           <-----chèn giá mua ở đây------>                  <-------chèn giá bán ở đây------>
        
        - Giả sử:
             _____________________            ______________________            ______________________           ______________________            ______________________           ______________________
price_list:  |  BOTTOM:          |            |  PriceItem:        |            |  PriceItem:        |           |  HEAD:             |            |  PriceItem:        |           |  TOP:              |
             |  + price: min(P2) |            |  + price: 1        |            |  + price: 2        |           |  + price: None     |            |  + price: 4        |           |  + price: max(P2)  |
             |  + prev: max(P2)  |<---------->|  + prev: min(P2)   |<---------->|  + prev: Some(1)   |<--------->|  + prev: Some(2)   |<---------->|  + prev: None      |<--------->|  + prev: Some(4)   |  
             |  + next: Some(1)  |            |  + next: Some(2)   |            |  + next: None      |           |  + next: Some(4)   |            |  + next: max(P2)   |           |  + next: min(P2)   |
             |  + ...            |            |  + ...             |            |  + ...             |           |  + ...             |            |  + ...             |           |  + ...             |            
             |___________________|            |____________________|            |____________________|           |____________________|            |____________________|           |____________________|

        - Giả sử user order 1 lệnh mới với 1 mức giá cụ thể: 
        => Nếu mức giá đó đã tồn tại trong list rồi thì mình chỉ cần lấy PriceItem đó ra, update lại buy_amount, sell_amount và orders thôi
        => Nếu mức giá đó chưa tồn tại trong list thì mình tạo một PriceItem mới với mức giá đó và chèn nó vào trong list

        Kiểu generic_type S là mapping từ token_hash => các mức giá => PriceItem tương ứng
    */ 

    // Đọc node head
    pub fn read_head(thash: P1) -> PriceItem<P1, P2, P3>{
        Self::read(thash, None)
    }

    #[allow(dead_code)]
    pub fn read_bottom(thash: P1) -> PriceItem<P1, P2, P3> {
        Self::read(thash, Some(P2::min_value()))
    }

    #[allow(dead_code)]
    pub fn read_top(thash: P1) -> PriceItem<P1, P2, P3> {
        Self::read(thash, Some(P2::max_value()))
    }

    // Get dữ liệu từ node, nếu chưa có thì khởi tạo 1 circular_linked_list mới rồi trả về node head
    pub fn read(thash: P1, price: Option<P2>) -> PriceItem<P1, P2, P3> {
        S::get(thash, price).unwrap_or_else(|| {
            let bottom = PriceItem {
                prev: Some(P2::max_value()),
                next: None,
                price: Some(P2::min_value()),
                orders: Vec::<P1>::new(),
                buy_amount: Default::default(),
                sell_amount: Default::default(),
            };

            let top = PriceItem {
                prev: None,
                next: Some(P2::min_value()),
                price: Some(P2::max_value()),
                orders: Vec::<P1>::new(),
                buy_amount: Default::default(),
                sell_amount: Default::default(),
            };

            let head = PriceItem {
                prev: Some(P2::min_value()),
                next: Some(P2::max_value()),
                price: None,
                orders: Vec::<P1>::new(),
                buy_amount: Default::default(),
                sell_amount: Default::default(),
            };

            Self::write(thash, bottom.price, bottom);
            Self::write(thash, top.price, top);
            Self::write(thash, head.price, head.clone());
            head
        })
    }

    // fn dùng để insert các mapping
    pub fn write(thash: P1, price: Option<P2>, item: PriceItem<P1, P2, P3>) {
        S::insert(thash, price, item);
    }

    // fn dùng để update khi có order
    pub fn append(thash: P1, price: P2, ohash: P1, sell_amount: P3, buy_amount: P3, otype: OrderType) {
        // lấy PriceItem từ list ra
        let item = S::get(thash, Some(price));

        match item {
            Some(mut item) => {  // Nếu PriceItem đã tồn tại, mình chỉ cần update amount và push order_hash vô
                item.orders.push(ohash);
                item.buy_amount += buy_amount;
                item.sell_amount += sell_amount;
                Self::write(thash, Some(price), item);
                return;
            },
            None => { // Nếu PriceItem chưa tồn tại mình sẽ tạo PriceItem mới và chèn nó vô list
                let start_item;
                let end_item;

                /* Lấy vị trí start và end để xác định PriceItem mới sẽ nằm trong khoảng nào 
                    + Buy  => nó nằm trong khoảng Bottom -> Head
                    + Sell => nó nằm trong khoảng Head   -> Top
                */ 
                match otype {
                    OrderType::Buy => {
                        start_item = Some(P2::min_value());
                        end_item = None;
                    },
                    OrderType::Sell => {
                        start_item = None;
                        end_item = Some(P2::max_value());
                    }
                }

                /*Chạy vòng while từ vị trí start đến end để tìm vị trí cần chèn PriceItem 
                    Giả sử: 
                    + Lệnh buy tại mức giá 3
                    + List hiện tại: Bottom -> 1 -> 2 -> 4 -> Head
                    => Vị trí cần chèn là tại 2
                    => Update lại next của 2, prev của 4 và chèn PriceItem mới vào với prev là 2 và next là 4
                */
                let mut item = Self::read(thash, start_item);
                while item.next != end_item {
                    match item.next {
                        Some(_price) => {
                            if price < _price {
                                break;
                            }
                        },
                        None => {}
                    }
                    item = Self::read(thash, item.next);
                }

                // update new_prev
                let new_prev = PriceItem {
                    next: Some(price),
                    ..item
                };
                Self::write(thash, new_prev.price, new_prev.clone());

                // update new_next
                let mut new_next = Self::read(thash, item.next);
                new_next.prev = Some(price);
                Self::write(thash, new_next.price, new_next.clone());

                // update new_item and insert it to list
                let mut v = Vec::new();
                v.push(ohash);
                let new_item = PriceItem {
                    prev: new_prev.price,
                    next: new_next.price,
                    buy_amount,
                    sell_amount,
                    orders: v,
                    price: Some(price),
                };
                Self::write(thash, new_item.price, new_item);
            }
        }
    }

    // trả về giá match tiếp theo, nếu mua thì là mức giá prev, còn bán là mức giá next
    pub fn next_match_price(price_item: &PriceItem<P1, P2, P3>, otype: OrderType) -> Option<P2>{
        if otype == OrderType::Buy {
            price_item.prev
        } else {
            price_item.next
        }
    }

    // update lại amount khi có order khớp lệnh => giảm amount
    pub fn update_reduce_amount(thash: P1, price: P2, sell_amount: P3, buy_amount: P3) {
        let mut item = Self::read(thash, Some(price)); 
        item.buy_amount = item.buy_amount - buy_amount;
        item.sell_amount = item.sell_amount - sell_amount;
        Self::write(thash, Some(price), item);
    }

    // fn remove 1 PriceItem khỏi list, take item đó ra và update lại pre, next của các item bên cạnh
    pub fn remove_item(thash: P1, price: P2) {
        if let Some(item) = S::take(thash, Some(price)) {
            S::mutate(thash, item.prev, |_item| {
                if let Some(x) = _item {
                    x.next = item.next;
                }
            });

            S::mutate(thash, item.next, |_item| {
                if let Some(x) = _item {
                    x.prev = item.prev;
                }
            });
        }
    }

    // fn remove order khỏi orders list
    pub fn remove_order(thash: P1, price: P2, ohash: P1, sell_amount: P3, buy_amount: P3) -> DispatchResult{
        let item = S::get(thash, Some(price));
        match item {
            Some(mut item) => {
                ensure!(item.orders.contains(&ohash), "Cancel the order but it not exists in orders list");
                item.orders.retain(|&o| o != ohash); // like filter
                item.buy_amount -= buy_amount;
                item.sell_amount -= sell_amount;

                // nếu list orders bằng 0 thì mình xóa luôn PriceItem này
                if item.orders.len() == 0 {
                    Self::remove_item(thash, price);
                }
                Self::write(thash, Some(price), item);
            },
            None => {} // ????
        }
        Ok(())
    }

    // fn remove order theo thứ tự từ trên xuống tại 1 PriceItem khi khớp giá
    pub fn remove_orders_in_one_item(thash: P1, price: P2) -> DispatchResult {
        match S::get(thash, Some(price)) {
            Some(mut item) => {
                while item.orders.len() > 0 {
                    let ohash = item.orders.get(0).ok_or("Cannot get order hash")?;
                    let order = trade::Orders::<T>::get(ohash.borrow()).ok_or("Cannot get order")?;
                    ensure!(order.is_finished(), "Try to remove not finished order");
                    item.orders.remove(0);
                    Self::write(thash, Some(price), item.clone());
                }

                if item.orders.len() == 0 {
                    Self::remove_item(thash, price);
                }
            },
            None => {}  // ????
        }
        Ok(())
    }

    // fn remove các order khớp giá, nó sẽ chạy liên tục hết PriceItem này sẽ chuyển đến PriceItem tiếp theo
    pub fn remove_order_match_price(thash: P1, otype: OrderType) {
        let end_item;

        if otype == OrderType::Buy {
            end_item = Some(P2::min_value()); // nếu buy thì giá khớp từ cao đến thấp
        } else {
            end_item = Some(P2::max_value()); // nếu sell thì giá khớp từ thấp đến cao
        }

        let mut head = Self::read_head(thash);

        loop {
            let price = Self::next_match_price(&head, otype.clone());
            if price == end_item {
                break;
            }

            match Self::remove_orders_in_one_item(thash, price.unwrap()) {
                Err(_) => break,
                _ => {}
            };

            head = Self::read_head(thash);
        }
    }
}
