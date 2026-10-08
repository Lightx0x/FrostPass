use anchor_lang::{
    prelude::*,
    system_program::{transfer, Transfer},
};
use mpl_core::{
    accounts::BaseAssetV1,
    instructions::BurnV1CpiBuilder,
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

// Burns a ticket with the event's permanent burn delegate (the EventConfig PDA signs with
// `signer_seeds`) and sends the burned asset's rent to `rent_recipient`.
// mpl-core refunds that rent to the burn payer, so if the payer isn't the recipient,
// the refunded amount is forwarded from the payer (who must be a signer) to the recipient.
#[allow(clippy::too_many_arguments)]
pub fn burn_ticket<'info>(
    mpl_core_program: &AccountInfo<'info>,
    ticket_asset: &AccountInfo<'info>,
    collection: &AccountInfo<'info>,
    event_config: &AccountInfo<'info>,
    payer: &AccountInfo<'info>,
    rent_recipient: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    signer_seeds: &[&[u8]],
) -> Result<()> {
    let payer_lamports_before = payer.lamports();

    BurnV1CpiBuilder::new(mpl_core_program)
        .asset(ticket_asset)
        .collection(Some(collection))
        .payer(payer)
        .authority(Some(event_config))
        .system_program(Some(system_program))
        .invoke_signed(&[signer_seeds])?;

    let asset_rent_refund = payer.lamports().saturating_sub(payer_lamports_before);

    if asset_rent_refund > 0 && payer.key != rent_recipient.key {
        transfer(
            CpiContext::new(
                *system_program.key,
                Transfer {
                    from: payer.clone(),
                    to: rent_recipient.clone(),
                },
            ),
            asset_rent_refund,
        )?;
    }

    Ok(())
}
