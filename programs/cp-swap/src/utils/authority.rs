use anchor_lang::prelude::*;
use crate::states::PoolState;

/// Validates that the provided authority matches the pool's configured authority
pub fn validate_authority(
    pool_state: &PoolState,
    authority: &Pubkey,
    program_id: &Pubkey,
) -> Result<()> {
    let expected_authority = pool_state.get_pool_authority(program_id);
    
    require!(
        authority == &expected_authority,
        crate::error::ErrorCode::InvalidAuthority
    );
    
    Ok(())
}

/// Check if the pool uses custom authority and if the signer matches
pub fn validate_custom_authority_signer(
    pool_state: &PoolState,
    authority: &Signer,
) -> Result<()> {
    if pool_state.is_custom_authority() {
        require!(
            authority.key() == pool_state.custom_authority,
            crate::error::ErrorCode::InvalidAuthority
        );
    }
    
    Ok(())
}

/// Gets the seeds for PDA authority signing
pub fn get_pda_authority_seeds(bump: u8) -> Vec<Vec<u8>> {
    vec![
        crate::AUTH_SEED.as_bytes().to_vec(),
        vec![bump],
    ]
}