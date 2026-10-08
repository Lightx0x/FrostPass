use anchor_lang::prelude::*;
use mpl_core::{
    accounts::BaseAssetV1,
    types::{Key, UpdateAuthority},
};

use crate::{constants::MAX_SCANNERS, error::ErrorCode, state::EventConfig};

// Deserializes a Core asset and checks it's a ticket of this event (member of its collection).
// The caller must still constrain the account with `owner = MPL_CORE_ID`.
pub fn load_ticket_asset(
    ticket_asset: &AccountInfo,
    event_config: &EventConfig,
) -> Result<BaseAssetV1> {
    let asset = BaseAssetV1::from_bytes(&ticket_asset.data.borrow())
        .map_err(|_| ErrorCode::InvalidEvent)?;

    // from_bytes doesn't check the account type, so reject collections and other Core accounts
    require!(asset.key == Key::AssetV1, ErrorCode::InvalidEvent);

    require!(
        asset.update_authority == UpdateAuthority::Collection(event_config.collection),
        ErrorCode::InvalidEvent
    );

    Ok(asset)
}

// Shared by init_event and update_scanners so the two can't drift.
pub fn validate_scanners(scanners: &[Pubkey]) -> Result<()> {
    require!(!scanners.is_empty(), ErrorCode::NoScannersProvided);
    require!(scanners.len() <= MAX_SCANNERS, ErrorCode::TooManyScanners);
    require!(
        !scanners.contains(&Pubkey::default()),
        ErrorCode::InvalidScanner
    );

    for scanner in 0..scanners.len() {
        for next_scanner in (scanner + 1)..scanners.len() {
            require!(
                scanners[scanner] != scanners[next_scanner],
                ErrorCode::DuplicateScanner
            );
        }
    }

    Ok(())
}
