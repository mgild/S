use flat_fee_interface::PriceLpTokensToRedeemKeys;
use solana_program::pubkey::Pubkey;

use crate::program;

pub struct PriceLpTokenToRedeemFreeArgs {
    pub output_lst_mint: Pubkey,
}

impl PriceLpTokenToRedeemFreeArgs {
    pub fn resolve(&self) -> PriceLpTokensToRedeemKeys {
        PriceLpTokensToRedeemKeys {
            output_lst_mint: self.output_lst_mint,
            state: program::STATE_ID,
        }
    }
}
