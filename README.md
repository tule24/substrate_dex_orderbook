# DEX Orderbook Exchange
![Rust](https://img.shields.io/badge/rust-%23000000.svg?style=for-the-badge&logo=rust&logoColor=white) ![Polkadot](https://img.shields.io/badge/polkadot-E6007A?style=for-the-badge&logo=polkadot&logoColor=white)

## Main function
### Pallet tokens
- `issue`: issue a token
- `do_issue`: helper function to issue token
- `transfer`: transfer token
- `do_transfer `: helper function to transfer token
- `do_freeze`: helper function to freeze balance of user when user creates order
- `do_unfreeze`: helper function to unfreeze balance of user when user cancels order
- `get_balance`: helper function to get user's balance
- `check_balance_enough`: helper function to make sure the balance is enough before it subtracted
- `check_balance_overflow`: helper function to make sure the balance isn't overflow before it added
- `update_balance`: helper function to update user's balance

### Pallet tokens
- `linked_price_list`: 
```
fn new => init một Circular Linked List cho mỗi token, cấu trúc sẽ là: 
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
```
- `create_trade_pair`: create a new create_trade_pair
- `do_create_trade_pair`: helper function to create tradepair
- `create_order`: create a limit order
- `do_create_limit_order`: helper function to create limit order
- `cancel_limit_order`: cancel order
- `do_create_market_order`: helper function to create market order
- `ensure_bounds` + `ensure_counterparty_amount_bounds` + `ensure_trade_pair`: make sure all require before process order match price 
- `order_match`: function to process orders that match the price
- `set_tp_market_data`: update tradepair market data
- `on_initialize` + `on_finalize`: function to update trade volume, total volume, last match price,...
