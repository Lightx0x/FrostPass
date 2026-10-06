use anchor_lang::prelude::*;
use mpl_core::{accounts::BaseAssetV1, types::UpdateAuthority};

use crate::{error::ErrorCode, state::EventConfig};

// Deserializes a Core asset and checks it's a ticket of this event (member of its collection).
// The caller must still constrain the account with `owner = MPL_CORE_ID`.
pub fn load_ticket_asset(
    ticket_asset: &AccountInfo,
    event_config: &EventConfig,
) -> Result<BaseAssetV1> {
    let asset = BaseAssetV1::from_bytes(&ticket_asset.data.borrow())
        .map_err(|_| ErrorCode::InvalidEvent)?;

    require!(
        asset.update_authority == UpdateAuthority::Collection(event_config.collection),
        ErrorCode::InvalidEvent
    );

    Ok(asset)
}
