#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::{
		pallet_prelude::*,
		sp_runtime::{
			traits::{Bounded, Hash},
			ArithmeticError,
		},
		sp_std::vec::Vec,
		traits::{Currency, Randomness},
		Blake2_128Concat,
	};
	use frame_system::pallet_prelude::*;
	use scale_info::TypeInfo;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	type Balance<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance; // Khai báo balance type

	#[derive(Clone, Encode, Decode, PartialEq, RuntimeDebug, TypeInfo)]
	#[scale_info(skip_type_params(T))]
	// Thông tin của 1 token gồm: hash, symbol và tổng supply
	pub struct Token<T: Config> {
		pub hash: T::Hash,
		pub symbol: Vec<u8>,
		pub total_supply: Balance<T>,
	}

	#[derive(Clone, Encode, Decode, PartialEq, RuntimeDebug, TypeInfo)]
	/* Chỉ là enum hỗ trợ để update balance nhanh hơn thôi. Ở đây mình có 3 loại balance (của 1 loại token cụ thể): 
		+ Balance: Tổng số dư của 1 user
		+ FreeBalance: Số dư có thể giao dịch
		+ FreezedBalance: Số dư bị đóng băng khi mà user đặt lệnh
		=> Balance = FreeBalance + FreezedBalance
	*/ 
	enum OptionBalance {
		Balance,
		FreeBalance,
		FreezedBalance,
	}

	#[pallet::config]
	pub trait Config: frame_system::Config {
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
		type Currency: Currency<Self::AccountId>; // dùng để định dạng type Balance
		type TokenRandom: Randomness<Self::Hash, Self::BlockNumber>; // dùng để tạo random
	}

	#[pallet::storage]
	#[pallet::getter(fn tokens)]
	// Mapping token_hash => token struct
	pub type Tokens<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, Token<T>>;

	#[pallet::storage]
	#[pallet::getter(fn owners)]
	// Mapping token_hash => owner (who issue tokens)
	pub type Owners<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, T::AccountId, OptionQuery>;

	#[pallet::storage]
	#[pallet::getter(fn balance_of)]
	// Mapping accountId => token_hash => balance
	pub type BalanceOf<T: Config> = StorageDoubleMap<_, Blake2_128Concat, T::AccountId, Blake2_128Concat, T::Hash, Balance<T>, ValueQuery,>;

	#[pallet::storage]
	#[pallet::getter(fn free_balance_of)]
	// Mapping accountId => token_hash => balance_free
	pub type FreeBalanceOf<T: Config> = StorageDoubleMap<_, Blake2_128Concat, T::AccountId, Blake2_128Concat, T::Hash, Balance<T>, ValueQuery,>;

	#[pallet::storage]
	#[pallet::getter(fn freezed_balance_of)]
	// Mapping accountId => token_hash => balance_freezed
	pub type FreezedBalanceOf<T: Config> = StorageDoubleMap<_, Blake2_128Concat, T::AccountId, Blake2_128Concat, T::Hash, Balance<T>, ValueQuery,>;

	#[pallet::storage]
	#[pallet::getter(fn owned_token_index)]
	// Mapping accountId => index => token_hash // Đánh index cho các token mà user đã issue ra
	pub type OwnedTokensIndex<T: Config> = StorageDoubleMap<_, Blake2_128Concat, T::AccountId, Blake2_128Concat, u64, T::Hash, OptionQuery,>;

	#[pallet::storage]
	#[pallet::getter(fn owned_token_total)]
	// Mapping accountId => total token that account issue // Tổng số loại token mà user issue (nó # với balance)
	pub type OwnedTokensTotal<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, u64, ValueQuery>;

	#[pallet::storage]
	#[pallet::getter(fn nonce)]
	// Nonce: tăng khi có 1 token issue => Tổng số loại token đã được issue
	pub type Nonce<T: Config> = StorageValue<_, u64, ValueQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		Issued { minter: T::AccountId, token_hash: T::Hash, total_supply: Balance<T> },					// when user issue a token success	
		Transferred { from: T::AccountId, to: T::AccountId, token_hash: T::Hash, amount: Balance<T> },  // when user transfer token success
		Freezed { owner: T::AccountId, token_hash: T::Hash, amount: Balance<T> },						// when user make an order
		Unfreezed { owner: T::AccountId, token_hash: T::Hash, amount: Balance<T> },						// when user cancel an order
	}

	#[pallet::error]
	pub enum Error<T> {
		NoMatchingToken,            // There is no match token
		BalanceNotEnough,           // The balance is not enough
		AmountOverFlow,             // Amount overflow
		SenderHaveNoToken,          // Sender does not have token
		MemoLengthExceedLimitation, // Memo length exceed limitation
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::weight(100)]  // fn issue a token 
		pub fn issue( origin: OriginFor<T>, symbol: Vec<u8>, total_supply: Balance<T> ) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			Self::do_issue(sender, symbol, total_supply)
		}

		#[pallet::weight(100)] // fn transfer a token 
		pub fn transfer( origin: OriginFor<T>, to: T::AccountId, token_hash: T::Hash, amount: Balance<T>, memo: Option<Vec<u8>> ) -> DispatchResult {
			let from = ensure_signed(origin)?;
			Self::do_transfer(from, to, token_hash, amount, memo)
		}
	}

	// Helper fn
	impl<T: Config> Pallet<T> {
		// fn when issue a token
		pub fn do_issue( sender: T::AccountId, symbol: Vec<u8>, total_supply: Balance<T> ) -> DispatchResult {
			// create a token struct
			let nonce = Self::nonce();
			let random = T::TokenRandom::random(&symbol).0;
			let token_hash = (random, sender.clone(), nonce).using_encoded(T::Hashing::hash);
			let token = Token::<T> { hash: token_hash.clone(), symbol: symbol.clone(), total_supply };

			// update nonce
			let new_nonce = nonce.checked_add(1).ok_or(ArithmeticError::Overflow)?;
			<Nonce<T>>::put(new_nonce);

			// update storage Tokens, Owners
			<Tokens<T>>::insert(token_hash.clone(), token);
			<Owners<T>>::insert(token_hash.clone(), sender.clone());

			// update balance, freeBalance of user
			Self::update_balance( sender.clone(), token_hash.clone(), total_supply, OptionBalance::Balance )?;
			Self::update_balance( sender.clone(), token_hash.clone(), total_supply, OptionBalance::FreeBalance )?;

			// update storage OwnedTokensIndex, OwnedTokensTotal
			let owned_token_index = Self::owned_token_total(sender.clone());
			<OwnedTokensIndex<T>>::insert(sender.clone(), owned_token_index, token_hash.clone());
			let owned_token_total = owned_token_index.checked_add(1).ok_or(ArithmeticError::Overflow)?;
			<OwnedTokensTotal<T>>::insert(sender.clone(), owned_token_total);
			
			Self::deposit_event(Event::Issued { minter: sender, token_hash, total_supply });
			Ok(())
		}

		// fn when transfer token
		pub fn do_transfer( from: T::AccountId, to: T::AccountId, token_hash: T::Hash, amount: Balance<T>, memo: Option<Vec<u8>> ) -> DispatchResult {
			// Check token_exist, sender_have_token
			Self::check_token_exist(token_hash.clone())?;
			Self::check_user_have_token(from.clone(), token_hash.clone())?;

			// Don't know why use memo???
			if let Some(memo) = memo {
				ensure!(memo.len() <= 512, <Error<T>>::MemoLengthExceedLimitation);
			}

			// check from_account enough balance and subtract it => balance, freeBalance is reduced
			let new_from_balance = Self::check_balance_enough( from.clone(), token_hash.clone(), amount, OptionBalance::Balance )?;
			let new_from_free_balance = Self::check_balance_enough( from.clone(), token_hash.clone(), amount, OptionBalance::FreeBalance )?;

			// check to_account not overflow when increase balance and add it => balance, freeBalance is increased
			let new_to_balance = Self::check_balance_overflow( to.clone(), token_hash.clone(), amount, OptionBalance::Balance )?;
			let new_to_free_balance = Self::check_balance_overflow( to.clone(), token_hash.clone(), amount, OptionBalance::FreeBalance )?;

			// update balance, freeBalance of from_account and to_account
			Self::update_balance( from.clone(), token_hash.clone(), new_from_balance, OptionBalance::Balance )?;
			Self::update_balance( from.clone(), token_hash.clone(), new_from_free_balance, OptionBalance::FreeBalance )?;
			Self::update_balance( to.clone(), token_hash.clone(), new_to_balance, OptionBalance::Balance )?;
			Self::update_balance( to.clone(), token_hash.clone(), new_to_free_balance, OptionBalance::FreeBalance )?;

			Self::deposit_event(Event::Transferred { from, to, token_hash, amount });
			Ok(())
		}

		// fn when make an order
		pub fn do_freeze( owner: T::AccountId, token_hash: T::Hash, amount: Balance<T> ) -> DispatchResult {
			Self::check_token_exist(token_hash.clone())?;
			Self::check_user_have_token(owner.clone(), token_hash.clone())?;

			// check free_balance enough to freezed => free_balance reduced, freezed_balance increased
			let new_free_balance = Self::check_balance_enough(owner.clone(), token_hash.clone(), amount, OptionBalance::FreeBalance )?;
			let new_freezed_balance = Self::check_balance_overflow( owner.clone(), token_hash.clone(), amount, OptionBalance::FreezedBalance )?;

			// update freeBalance, freezedBalance of user
			Self::update_balance( owner.clone(), token_hash.clone(), new_free_balance, OptionBalance::FreeBalance )?;
			Self::update_balance( owner.clone(), token_hash.clone(), new_freezed_balance, OptionBalance::FreezedBalance )?;

			Self::deposit_event(Event::Freezed { owner, token_hash, amount });
			Ok(())
		}

		// fn when cancel an order
		pub fn do_unfreeze( owner: T::AccountId, token_hash: T::Hash, amount: Balance<T> ) -> DispatchResult {
			Self::check_token_exist(token_hash.clone())?;
			Self::check_user_have_token(owner.clone(), token_hash.clone())?;

			// contrast with do_free
			let new_free_balance = Self::check_balance_overflow( owner.clone(), token_hash.clone(), amount, OptionBalance::FreeBalance )?;
			let new_freezed_balance = Self::check_balance_enough( owner.clone(), token_hash.clone(), amount, OptionBalance::FreezedBalance )?;

			// update freeBalance, freezedBalance of user
			Self::update_balance( owner.clone(), token_hash.clone(), new_free_balance, OptionBalance::FreeBalance )?;
			Self::update_balance( owner.clone(), token_hash.clone(), new_freezed_balance, OptionBalance::FreezedBalance )?;

			Self::deposit_event(Event::Unfreezed { owner, token_hash, amount });
			Ok(())
		}

		// fn make sure free_balance enough
		pub fn ensure_free_balance( sender: T::AccountId, token_hash: T::Hash, amount: Balance<T> ) -> DispatchResult {
			Self::check_token_exist(token_hash.clone())?;
			Self::check_user_have_token(sender.clone(), token_hash.clone())?;

			let free_balance = Self::free_balance_of(sender, token_hash);
			ensure!(free_balance >= amount, <Error<T>>::BalanceNotEnough);
			Ok(())
		}

		// fn check_token_exist
		fn check_token_exist(token_hash: T::Hash) -> Result<(), Error<T>> {
			let token = Self::tokens(token_hash);
			match token {
				None => Err(<Error<T>>::NoMatchingToken),
				_ => Ok(()),
			}
		}

		// fn check_user_have_token
		fn check_user_have_token( user: T::AccountId, token_hash: T::Hash ) -> Result<(), Error<T>> {
			let token = <FreeBalanceOf<T>>::contains_key(user, token_hash);
			if !token {
				return Err(<Error<T>>::SenderHaveNoToken);
			}
			Ok(())
		}

		// fn get_balance
		fn get_balance( owner: T::AccountId, token_hash: T::Hash, option_balance: OptionBalance ) -> Balance<T> {
			match option_balance {
				OptionBalance::Balance => Self::balance_of(owner, token_hash),
				OptionBalance::FreeBalance => Self::free_balance_of(owner, token_hash),
				OptionBalance::FreezedBalance => Self::freezed_balance_of(owner, token_hash),
			}
		}

		// fn check balance enough and subtract it and return new_balance
		fn check_balance_enough( owner: T::AccountId, token_hash: T::Hash, amount: Balance<T>, option_balance: OptionBalance ) -> Result<Balance<T>, Error<T>> {
			let mut balance = Self::get_balance(owner, token_hash, option_balance);
			if balance >= amount {
				balance -= amount;
				return Ok(balance);
			}
			return Err(<Error<T>>::BalanceNotEnough);
		}

		// fn check balance not overflow when increase and add to it and return new_balance
		fn check_balance_overflow( owner: T::AccountId, token_hash: T::Hash, amount: Balance<T>, option_balance: OptionBalance ) -> Result<Balance<T>, Error<T>> {
			let mut balance = Self::get_balance(owner, token_hash, option_balance);
			if balance + amount <= <Balance<T>>::max_value() {
				balance += amount;
				return Ok(balance);
			}
			return Err(<Error<T>>::AmountOverFlow);
		}

		// fn update balance depends on OptionBalance
		fn update_balance( owner: T::AccountId, token_hash: T::Hash, balance: Balance<T>, option_balance: OptionBalance ) -> Result<(), Error<T>> {
			match option_balance {
				OptionBalance::Balance => <BalanceOf<T>>::insert(owner, token_hash, balance),
				OptionBalance::FreeBalance => <FreeBalanceOf<T>>::insert(owner, token_hash, balance),
				OptionBalance::FreezedBalance => <FreezedBalanceOf<T>>::insert(owner, token_hash, balance),
			};
			Ok(())
		}
	}
}
