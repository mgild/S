use s_controller_interface::{
    swap_exact_in_ix, SControllerError, SwapExactInIxArgs, SwapExactInIxData, SwapExactInKeys,
};
use solana_program::{
    instruction::{AccountMeta, Instruction},
    program_error::ProgramError,
    pubkey::Pubkey,
};
use solana_readonly_account::{ReadonlyAccountData, ReadonlyAccountOwner, ReadonlyAccountPubkey};

use crate::{
    index_to_u32, ix_extend_with_pricing_program_price_swap_accounts,
    ix_extend_with_src_dst_sol_value_calculator_accounts, SrcDstLstIndexes,
    SrcDstLstSolValueCalcAccounts, SrcDstLstSolValueCalcExtendCount, SwapByMintsFreeArgs,
};

#[derive(Clone, Copy, Debug)]
pub struct SwapExactInIxFullArgs {
    pub src_lst_index: usize,
    pub dst_lst_index: usize,
    pub min_amount_out: u64,
    pub amount: u64,
}

pub fn swap_exact_in_ix_full<K: Into<SwapExactInKeys>>(
    accounts: K,
    SwapExactInIxFullArgs {
        src_lst_index,
        dst_lst_index,
        min_amount_out,
        amount,
    }: SwapExactInIxFullArgs,
    sol_val_calc_accounts: SrcDstLstSolValueCalcAccounts,
    pricing_program_accounts: &[AccountMeta],
    pricing_program_id: Pubkey,
) -> Result<Instruction, ProgramError> {
    let src_lst_index = index_to_u32(src_lst_index)?;
    let dst_lst_index = index_to_u32(dst_lst_index)?;
    let mut ix = swap_exact_in_ix(
        accounts,
        SwapExactInIxArgs {
            src_lst_value_calc_accs: 0,
            dst_lst_value_calc_accs: 0,
            src_lst_index,
            dst_lst_index,
            min_amount_out,
            amount,
        },
    )?;
    let SrcDstLstSolValueCalcExtendCount {
        src_lst: src_lst_value_calc_accs,
        dst_lst: dst_lst_value_calc_accs,
    } = ix_extend_with_src_dst_sol_value_calculator_accounts(&mut ix, sol_val_calc_accounts)
        .map_err(|_e| SControllerError::MathError)?;
    ix_extend_with_pricing_program_price_swap_accounts(
        &mut ix,
        pricing_program_accounts,
        pricing_program_id,
    )
    .map_err(|_e| SControllerError::MathError)?;
    // TODO: better way to update *_calc_accs than double serialization here
    let mut overwrite = &mut ix.data[..];
    SwapExactInIxData(SwapExactInIxArgs {
        src_lst_value_calc_accs,
        dst_lst_value_calc_accs,
        src_lst_index,
        dst_lst_index,
        min_amount_out,
        amount,
    })
    .serialize(&mut overwrite)?;
    Ok(ix)
}

#[derive(Clone, Copy, Debug)]
pub struct SwapExactInAmounts {
    pub min_amount_out: u64,
    pub amount: u64,
}

pub fn swap_exact_in_ix_by_mint_full<
    SM: ReadonlyAccountOwner + ReadonlyAccountPubkey,
    DM: ReadonlyAccountOwner + ReadonlyAccountPubkey,
    L: ReadonlyAccountData,
>(
    free_args: SwapByMintsFreeArgs<SM, DM, L>,
    SwapExactInAmounts {
        min_amount_out,
        amount,
    }: SwapExactInAmounts,
    sol_val_calc_accounts: SrcDstLstSolValueCalcAccounts,
    pricing_program_accounts: &[AccountMeta],
    pricing_program_id: Pubkey,
) -> Result<Instruction, ProgramError> {
    let (
        keys,
        SrcDstLstIndexes {
            src_lst_index,
            dst_lst_index,
        },
    ) = free_args.resolve_exact_in()?;
    let ix = swap_exact_in_ix_full(
        keys,
        SwapExactInIxFullArgs {
            src_lst_index,
            dst_lst_index,
            min_amount_out,
            amount,
        },
        sol_val_calc_accounts,
        pricing_program_accounts,
        pricing_program_id,
    )?;
    Ok(ix)
}
