use flat_fee_interface::{FlatFeeError, ProgramState, SetLpWithdrawalFeeKeys};
use solana_program::pubkey::Pubkey;
use solana_readonly_account::{ReadonlyAccountData, ReadonlyAccountPubkeyBytes};

use crate::{pda::ProgramStateFindPdaArgs, program as flat_fee_program, utils::try_program_state};

pub struct SetLpWithdrawalFeeFreeArgs<S: ReadonlyAccountPubkeyBytes + ReadonlyAccountData> {
    pub state_acc: S,
}

impl<S: ReadonlyAccountPubkeyBytes + ReadonlyAccountData> SetLpWithdrawalFeeFreeArgs<S> {
    pub fn resolve(self) -> Result<SetLpWithdrawalFeeKeys, FlatFeeError> {
        self.resolve_inner(flat_fee_program::STATE_ID)
    }

    pub fn resolve_for_prog(
        self,
        program_id: Pubkey,
    ) -> Result<SetLpWithdrawalFeeKeys, FlatFeeError> {
        let state_id = ProgramStateFindPdaArgs { program_id }
            .get_program_state_address_and_bump_seed()
            .0;

        self.resolve_inner(state_id)
    }

    fn resolve_inner(self, state_id: Pubkey) -> Result<SetLpWithdrawalFeeKeys, FlatFeeError> {
        let SetLpWithdrawalFeeFreeArgs { state_acc } = self;

        if state_acc.pubkey_bytes() != state_id.to_bytes() {
            return Err(FlatFeeError::IncorrectProgramState);
        }

        let bytes = &state_acc.data();
        let state: &ProgramState = try_program_state(bytes)?;

        Ok(SetLpWithdrawalFeeKeys {
            manager: state.manager,
            state: state_id,
        })
    }
}
