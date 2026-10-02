pub mod constants;
pub mod error;
pub mod instructions;
pub mod state;

use anchor_lang::prelude::*;

pub use constants::*;
pub use instructions::*;
pub use state::*;

declare_id!("8GqPBbxtkA9PbQYmAfb4SrU56tHiYaBh5kPUsHtM2hjk");

#[program]
pub mod frost_pass {
    use super::*;

    
}
