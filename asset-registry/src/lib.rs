#![cfg_attr(not(feature = "std"), no_std)]
// Older clippy versions give a false positive on the expansion of [pallet::call].
// This is fixed in https://github.com/rust-lang/rust-clippy/issues/8321
#![allow(clippy::large_enum_variant)]

use frame_support::{pallet_prelude::*, traits::EnsureOrigin, transactional};
use frame_system::pallet_prelude::*;
use orml_traits::asset_registry::AssetProcessor;
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{AtLeast32BitUnsigned, Member},
	DispatchResult,
};
use sp_std::prelude::*;
use xcm::{v2::prelude::*, VersionedMultiLocation};

pub use impls::*;
pub use module::*;
pub use weights::WeightInfo;

mod impls;
mod mock;
mod tests;
mod weights;

/// Data describing the asset properties.
#[derive(scale_info::TypeInfo, Encode, Decode, Clone, Eq, PartialEq, RuntimeDebug)]
pub struct AssetMetadata<Balance, CustomMetadata: Parameter + Member + TypeInfo> {
	pub decimals: u32,
	pub name: Vec<u8>,
	pub symbol: Vec<u8>,
	pub existential_deposit: Balance,
	pub location: Option<VersionedMultiLocation>,
	pub additional: CustomMetadata,
}

#[frame_support::pallet]
pub mod module {
	use super::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// Additional non-standard metadata to store for each asset
		type CustomMetadata: Parameter + Member + TypeInfo;

		/// The type used as a unique asset id,
		type AssetId: Parameter + Member + Default + TypeInfo;

		/// The origin that is allowed to manipulate metadata.
		type AuthorityOrigin: EnsureOrigin<<Self as frame_system::Config>::Origin>;

		/// A filter ran upon metadata registration that assigns an is and
		/// potentially modifies the supplied metadata.
		type AssetProcessor: AssetProcessor<Self::AssetId, AssetMetadata<Self::Balance, Self::CustomMetadata>>;

		/// The balance type.
		type Balance: Parameter + Member + AtLeast32BitUnsigned + Default + Copy;

		/// Weight information for extrinsics in this module.
		type WeightInfo: WeightInfo;
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Asset was not found.
		AssetNotFound,
		/// The version of the `VersionedMultiLocation` value used is not able
		/// to be interpreted.
		BadVersion,
		/// The asset id is invalid.
		InvalidAssetId,
		/// Another asset was already register with this location.
		ConflictingLocation,
		/// Another asset was already register with this asset id.
		ConflictingAssetId,
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(crate) fn deposit_event)]
	pub enum Event<T: Config> {
		RegisteredAsset {
			asset_id: T::AssetId,
			metadata: AssetMetadata<T::Balance, T::CustomMetadata>,
		},
		UpdatedAsset {
			asset_id: T::AssetId,
			metadata: AssetMetadata<T::Balance, T::CustomMetadata>,
		},
		SetLocation {
			asset_id: T::AssetId,
			location: Box<VersionedMultiLocation>,
		},
	}

	/// The metadata of an asset, indexed by asset id.
	#[pallet::storage]
	#[pallet::getter(fn metadata)]
	pub type Metadata<T: Config> =
		StorageMap<_, Twox64Concat, T::AssetId, AssetMetadata<T::Balance, T::CustomMetadata>, OptionQuery>;

	/// Maps a multilocation to an asset id - useful when processing xcm
	/// messages.
	#[pallet::storage]
	#[pallet::getter(fn location_to_asset_id)]
	pub type LocationToAssetId<T: Config> = StorageMap<_, Twox64Concat, MultiLocation, T::AssetId, OptionQuery>;

	/// The last processed asset id - used when assigning a sequential id.
	#[pallet::storage]
	#[pallet::getter(fn last_asset_id)]
	pub(crate) type LastAssetId<T: Config> = StorageValue<_, T::AssetId, ValueQuery>;

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		_phantom: PhantomData<T>,
	}

	#[cfg(feature = "std")]
	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			Self {
				_phantom: Default::default(),
			}
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {}
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::hooks]
	impl<T: Config> Hooks<T::BlockNumber> for Pallet<T> {}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::weight(T::WeightInfo::register_asset())]
		#[transactional]
		pub fn register_asset(
			origin: OriginFor<T>,
			metadata: AssetMetadata<T::Balance, T::CustomMetadata>,
			asset_id: Option<T::AssetId>,
		) -> DispatchResult {
			T::AuthorityOrigin::ensure_origin(origin)?;

			Self::do_register_asset(metadata, asset_id)
		}

		#[allow(clippy::too_many_arguments)]
		#[pallet::weight(T::WeightInfo::update_asset())]
		#[transactional]
		pub fn update_asset(
			origin: OriginFor<T>,
			asset_id: T::AssetId,
			decimals: Option<u32>,
			name: Option<Vec<u8>>,
			symbol: Option<Vec<u8>>,
			existential_deposit: Option<T::Balance>,
			location: Option<Option<VersionedMultiLocation>>,
			additional: Option<T::CustomMetadata>,
		) -> DispatchResult {
			T::AuthorityOrigin::ensure_origin(origin)?;

			Self::do_update_asset(
				asset_id,
				decimals,
				name,
				symbol,
				existential_deposit,
				location,
				additional,
			)?;

			Ok(())
		}
	}
}

impl<T: Config> Pallet<T> {
	/// Register a new asset
	pub fn do_register_asset(
		metadata: AssetMetadata<T::Balance, T::CustomMetadata>,
		asset_id: Option<T::AssetId>,
	) -> DispatchResult {
		let (asset_id, metadata) = T::AssetProcessor::pre_register(asset_id, metadata)?;

		Self::do_register_asset_without_asset_processor(metadata.clone(), asset_id.clone())?;

		T::AssetProcessor::post_register(asset_id, metadata)?;

		Ok(())
	}

	/// Like do_register_asset, but without calling pre_register and
	/// post_register hooks.
	/// This function is useful in tests but it might also come in useful to
	/// users.
	pub fn do_register_asset_without_asset_processor(
		metadata: AssetMetadata<T::Balance, T::CustomMetadata>,
		asset_id: T::AssetId,
	) -> DispatchResult {
		Metadata::<T>::try_mutate(&asset_id, |maybe_metadata| -> DispatchResult {
			// make sure this asset id has not been registered yet
			ensure!(maybe_metadata.is_none(), Error::<T>::ConflictingAssetId);

			*maybe_metadata = Some(metadata.clone());

			if let Some(ref location) = metadata.location {
				Self::do_insert_location(asset_id.clone(), location.clone())?;
			}

			Ok(())
		})?;

		Self::deposit_event(Event::<T>::RegisteredAsset { asset_id, metadata });

		Ok(())
	}

	pub fn do_update_asset(
		asset_id: T::AssetId,
		decimals: Option<u32>,
		name: Option<Vec<u8>>,
		symbol: Option<Vec<u8>>,
		existential_deposit: Option<T::Balance>,
		location: Option<Option<VersionedMultiLocation>>,
		additional: Option<T::CustomMetadata>,
	) -> DispatchResult {
		Metadata::<T>::try_mutate(&asset_id, |maybe_metadata| -> DispatchResult {
			let metadata = maybe_metadata.as_mut().ok_or(Error::<T>::AssetNotFound)?;
			if let Some(decimals) = decimals {
				metadata.decimals = decimals;
			}

			if let Some(name) = name {
				metadata.name = name;
			}

			if let Some(symbol) = symbol {
				metadata.symbol = symbol;
			}

			if let Some(existential_deposit) = existential_deposit {
				metadata.existential_deposit = existential_deposit;
			}

			if let Some(location) = location {
				Self::do_update_location(asset_id.clone(), metadata.location.clone(), location.clone())?;
				metadata.location = location;
			}

			if let Some(additional) = additional {
				metadata.additional = additional;
			}

			Self::deposit_event(Event::<T>::UpdatedAsset {
				asset_id: asset_id.clone(),
				metadata: metadata.clone(),
			});

			Ok(())
		})?;

		Ok(())
	}

	pub fn fetch_metadata_by_location(
		location: &MultiLocation,
	) -> Option<AssetMetadata<T::Balance, T::CustomMetadata>> {
		let asset_id = LocationToAssetId::<T>::get(location)?;
		Metadata::<T>::get(asset_id)
	}

	pub fn multilocation(asset_id: &T::AssetId) -> Result<Option<MultiLocation>, DispatchError> {
		Metadata::<T>::get(asset_id)
			.and_then(|metadata| {
				metadata
					.location
					.map(|location| location.try_into().map_err(|()| Error::<T>::BadVersion.into()))
			})
			.transpose()
	}

	/// update LocationToAssetId mapping if the location changed
	fn do_update_location(
		asset_id: T::AssetId,
		old_location: Option<VersionedMultiLocation>,
		new_location: Option<VersionedMultiLocation>,
	) -> DispatchResult {
		// Update `LocationToAssetId` only if location changed
		if new_location != old_location {
			// remove the old location lookup if it exists
			if let Some(ref old_location) = old_location {
				let location: MultiLocation = old_location.clone().try_into().map_err(|()| Error::<T>::BadVersion)?;
				LocationToAssetId::<T>::remove(location);
			}

			// insert new location
			if let Some(ref new_location) = new_location {
				Self::do_insert_location(asset_id, new_location.clone())?;
			}
		}

		Ok(())
	}

	/// insert location into the LocationToAssetId map
	fn do_insert_location(asset_id: T::AssetId, location: VersionedMultiLocation) -> DispatchResult {
		// if the metadata contains a location, set the LocationToAssetId
		let location: MultiLocation = location.try_into().map_err(|()| Error::<T>::BadVersion)?;
		LocationToAssetId::<T>::try_mutate(&location, |maybe_asset_id| {
			ensure!(maybe_asset_id.is_none(), Error::<T>::ConflictingLocation);
			*maybe_asset_id = Some(asset_id);
			Ok(())
		})
	}
}
