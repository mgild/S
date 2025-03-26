use generic_pool_calculator_interface::{GenericPoolCalculatorError, UpdateLastUpgradeSlotKeys};
use solana_readonly_account::{ReadonlyAccountData, ReadonlyAccountPubkeyBytes};

use crate::{
    utils::{read_programdata_addr, try_calculator_state},
    GenericPoolSolValCalc,
};

pub struct UpdateLastUpgradeSlotFreeArgs<
    S: ReadonlyAccountPubkeyBytes + ReadonlyAccountData,
    Q: ReadonlyAccountPubkeyBytes + ReadonlyAccountData,
> {
    pub state: S,
    pub pool_program: Q,
}

impl<
        S: ReadonlyAccountPubkeyBytes + ReadonlyAccountData,
        Q: ReadonlyAccountPubkeyBytes + ReadonlyAccountData,
    > UpdateLastUpgradeSlotFreeArgs<S, Q>
{
    pub fn resolve<P: GenericPoolSolValCalc>(
        self,
    ) -> Result<UpdateLastUpgradeSlotKeys, GenericPoolCalculatorError> {
        if self.state.pubkey_bytes() != P::CALCULATOR_STATE_PDA.to_bytes() {
            return Err(GenericPoolCalculatorError::WrongCalculatorStatePda);
        }
        if self.pool_program.pubkey_bytes() != P::POOL_PROGRAM_ID.to_bytes() {
            return Err(GenericPoolCalculatorError::WrongPoolProgram);
        }

        let state_bytes = &self.state.data();
        let calc_state = try_calculator_state(state_bytes)?;

        let pool_program_data = read_programdata_addr(&self.pool_program)?;

        Ok(UpdateLastUpgradeSlotKeys {
            manager: calc_state.manager,
            state: P::CALCULATOR_STATE_PDA,
            pool_program: P::POOL_PROGRAM_ID,
            pool_program_data,
        })
    }
}

/// Struct that uses defined const for POOL_PROGRAM_PROGDATA
/// so that it can be used without fetching POOL_PROGRAM
pub struct UpdateLastUpgradeSlotFreeArgsConst<S: ReadonlyAccountPubkeyBytes + ReadonlyAccountData> {
    pub state: S,
}

impl<S: ReadonlyAccountPubkeyBytes + ReadonlyAccountData> UpdateLastUpgradeSlotFreeArgsConst<S> {
    pub fn resolve<P: GenericPoolSolValCalc>(
        self,
    ) -> Result<UpdateLastUpgradeSlotKeys, GenericPoolCalculatorError> {
        if self.state.pubkey_bytes() != P::CALCULATOR_STATE_PDA.to_bytes() {
            return Err(GenericPoolCalculatorError::WrongCalculatorStatePda);
        }

        let state_bytes = &self.state.data();
        let calc_state = try_calculator_state(state_bytes)?;

        Ok(UpdateLastUpgradeSlotKeys {
            manager: calc_state.manager,
            state: P::CALCULATOR_STATE_PDA,
            pool_program: P::POOL_PROGRAM_ID,
            pool_program_data: P::POOL_PROGRAM_PROGDATA_ID,
        })
    }
}
