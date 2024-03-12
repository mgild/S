use anyhow::anyhow;
use jupiter_amm_interface::{
    AccountMap, Amm, KeyedAccount, Quote, QuoteParams, SwapAndAccountMetas, SwapParams,
};
use pricing_programs_interface::{PriceExactInIxArgs, PriceExactInKeys};
use rust_decimal::{prelude::FromPrimitive, Decimal};
use s_controller_interface::{LstState, PoolState, SControllerError};
use s_controller_lib::{
    calc_swap_protocol_fees, find_lst_state_list_address, find_pool_state_address,
    swap_exact_in_ix_by_mint_full, sync_sol_value_with_retval, try_lst_state_list, try_pool_state,
    CalcSwapProtocolFeesArgs, SrcDstLstSolValueCalcAccountSuffixes, SwapByMintsFreeArgs,
    SwapExactInAmounts,
};
use s_pricing_prog_aggregate::{KnownPricingProg, MutablePricingProg, PricingProg};
use s_sol_val_calc_prog_aggregate::{
    KnownLstSolValCalc, LidoLstSolValCalc, LstSolValCalc, MarinadeLstSolValCalc,
    MutableLstSolValCalc, SanctumSplLstSolValCalc, SplLstSolValCalc, SplLstSolValCalcInitKeys,
    WsolLstSolValCalc,
};
use sanctum_associated_token_lib::{CreateAtaAddressArgs, FindAtaAddressArgs};
use sanctum_lst_list::{PoolInfo, SanctumLst, SanctumLstList, SplPoolAccounts};
use sanctum_token_lib::{mint_supply, token_account_balance, MintWithTokenProgram};
use sanctum_token_ratio::{AmtsAfterFee, AmtsAfterFeeBuilder};
use solana_program::pubkey::{Pubkey, PubkeyError};
use solana_sdk::{account::Account, instruction::Instruction};
use std::str::FromStr;

pub const LABEL: &str = "Sanctum Infinity";

#[derive(Debug, Clone)]
pub struct LstData {
    pub sol_val_calc: KnownLstSolValCalc,
    pub reserves_balance: Option<u64>,
    pub token_program: Pubkey,
}

#[derive(Debug, Clone)]
pub struct SPoolJup {
    pub program_id: Pubkey,
    pub lst_state_list_addr: Pubkey,
    pub pool_state_addr: Pubkey,
    pub lp_mint_supply: Option<u64>,
    // keep as raw Account to use with solana-readonly-account traits
    pub pool_state_account: Option<Account>,
    pub lst_state_list_account: Account,
    pub pricing_prog: Option<KnownPricingProg>,
    // indices match that of lst_state_list.
    // None means we don't know how to handle the given lst
    // this could be due to incomplete data or unknown LST sol value calculator program
    pub lst_data_list: Vec<Option<LstData>>,
}

impl Default for SPoolJup {
    fn default() -> Self {
        Self {
            program_id: s_controller_lib::program::ID,
            lst_state_list_addr: s_controller_lib::program::LST_STATE_LIST_ID,
            pool_state_addr: s_controller_lib::program::POOL_STATE_ID,
            lp_mint_supply: None,
            pool_state_account: None,
            pricing_prog: None,
            lst_state_list_account: Account::default(),
            lst_data_list: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SPoolInitAccounts {
    pub lst_state_list: Pubkey,
    pub pool_state: Pubkey,
}

impl From<SPoolInitAccounts> for [Pubkey; 2] {
    fn from(
        SPoolInitAccounts {
            lst_state_list,
            pool_state,
        }: SPoolInitAccounts,
    ) -> Self {
        [lst_state_list, pool_state]
    }
}

impl Default for SPoolInitAccounts {
    fn default() -> Self {
        Self {
            lst_state_list: s_controller_lib::program::LST_STATE_LIST_ID,
            pool_state: s_controller_lib::program::POOL_STATE_ID,
        }
    }
}

impl SPoolJup {
    pub fn pool_state(&self) -> anyhow::Result<&PoolState> {
        let pool_state = self
            .pool_state_account
            .as_ref()
            .ok_or_else(|| anyhow!("Pool state not fetched"))?;
        Ok(try_pool_state(&pool_state.data)?)
    }

    pub fn lst_state_list(&self) -> anyhow::Result<&[LstState]> {
        Ok(try_lst_state_list(&self.lst_state_list_account.data)?)
    }

    pub fn pricing_prog(&self) -> anyhow::Result<&KnownPricingProg> {
        self.pricing_prog
            .as_ref()
            .ok_or_else(|| anyhow!("pricing program not fetched"))
    }

    /// Gets the list of accounts that must be fetched first to initialize
    /// SPool by passing the result into [`Self::from_fetched_accounts`]
    pub fn init_accounts(program_id: Pubkey) -> SPoolInitAccounts {
        SPoolInitAccounts {
            lst_state_list: find_lst_state_list_address(program_id).0,
            pool_state: find_pool_state_address(program_id).0,
        }
    }

    pub fn from_lst_state_list_account(
        program_id: Pubkey,
        lst_state_list_account: Account,
        lst_list: &[SanctumLst],
    ) -> anyhow::Result<Self> {
        let SPoolInitAccounts {
            lst_state_list: lst_state_list_addr,
            pool_state: pool_state_addr,
        } = Self::init_accounts(program_id);
        let lst_state_list = try_lst_state_list(&lst_state_list_account.data)?;
        let lst_data_list = lst_state_list
            .iter()
            .map(|lst_state| try_lst_data(lst_list, lst_state))
            .collect();
        Ok(Self {
            program_id,
            lst_state_list_addr,
            pool_state_addr,
            pool_state_account: None,
            pricing_prog: None,
            lp_mint_supply: None,
            lst_state_list_account,
            lst_data_list,
        })
    }

    pub fn from_fetched_accounts(
        program_id: Pubkey,
        accounts: &AccountMap,
        lst_list: &[SanctumLst],
    ) -> anyhow::Result<Self> {
        let SPoolInitAccounts {
            lst_state_list: lst_state_list_addr,
            pool_state: pool_state_addr,
        } = Self::init_accounts(program_id);

        let lst_state_list_acc = accounts
            .get(&lst_state_list_addr)
            .ok_or_else(|| anyhow!("Missing LST state list {lst_state_list_addr}"))?;
        let lst_state_list = Vec::from(try_lst_state_list(&lst_state_list_acc.data)?);
        let pool_state_acc = accounts
            .get(&pool_state_addr)
            .ok_or_else(|| anyhow!("Missing pool state {pool_state_addr}"))?;
        let pool_state = try_pool_state(&pool_state_acc.data)?;
        let pricing_prog = try_pricing_prog(pool_state, &lst_state_list)?;

        let mut res =
            Self::from_lst_state_list_account(program_id, lst_state_list_acc.clone(), lst_list)?;
        res.pool_state_account = Some(pool_state_acc.clone());
        res.pricing_prog = Some(pricing_prog);
        Ok(res)
    }

    pub fn pool_reserves_account(
        &self,
        LstState {
            mint,
            pool_reserves_bump,
            ..
        }: &LstState,
        LstData { token_program, .. }: &LstData,
    ) -> Result<Pubkey, PubkeyError> {
        CreateAtaAddressArgs {
            find_ata_args: FindAtaAddressArgs {
                wallet: self.pool_state_addr,
                mint: *mint,
                token_program: *token_program,
            },
            bump: *pool_reserves_bump,
        }
        .create_ata_address()
    }

    pub fn quote_swap_exact_in(
        &self,
        QuoteParams {
            amount,
            input_mint,
            output_mint,
            swap_mode: _,
        }: &QuoteParams,
    ) -> anyhow::Result<Quote> {
        let pool_state = self.pool_state()?;
        let pricing_prog = self
            .pricing_prog
            .as_ref()
            .ok_or_else(|| anyhow!("pricing program not fetched"))?;

        let (input_lst_state, input_lst_data) = self.find_ready_lst(*input_mint)?;
        let (pool_state, _input_lst_state, _input_reserves_balance) =
            apply_sync_sol_value(*pool_state, *input_lst_state, input_lst_data)?;
        let (output_lst_state, output_lst_data) = self.find_ready_lst(*output_mint)?;
        let (pool_state, _output_lst_state, output_reserves_balance) =
            apply_sync_sol_value(pool_state, *output_lst_state, output_lst_data)?;

        let in_sol_value = input_lst_data.sol_val_calc.lst_to_sol(*amount)?.get_min();
        if in_sol_value == 0 {
            return Err(SControllerError::ZeroValue.into());
        }
        let out_sol_value = pricing_prog.quote_exact_in(
            PriceExactInKeys {
                input_lst_mint: *input_mint,
                output_lst_mint: *output_mint,
            },
            &PriceExactInIxArgs {
                amount: *amount,
                sol_value: in_sol_value,
            },
        )?;
        if out_sol_value > in_sol_value {
            return Err(SControllerError::PoolWouldLoseSolValue.into());
        }
        let dst_lst_out = output_lst_data
            .sol_val_calc
            .sol_to_lst(out_sol_value)?
            .get_min();
        if dst_lst_out == 0 {
            return Err(SControllerError::ZeroValue.into());
        }
        let to_protocol_fees_lst_amount = calc_swap_protocol_fees(CalcSwapProtocolFeesArgs {
            in_sol_value,
            out_sol_value,
            dst_lst_out,
            trading_protocol_fee_bps: pool_state.trading_protocol_fee_bps,
        })?;
        let total_dst_lst_out = dst_lst_out
            .checked_add(to_protocol_fees_lst_amount)
            .ok_or(SControllerError::MathError)?;
        let not_enough_liquidity = total_dst_lst_out > output_reserves_balance;
        let (fee_amount, fee_pct) = calc_quote_fees(
            AmtsAfterFeeBuilder::new_amt_bef_fee(in_sol_value).with_amt_aft_fee(out_sol_value)?,
            &output_lst_data.sol_val_calc,
        )?;
        Ok(Quote {
            not_enough_liquidity,
            min_in_amount: None,
            min_out_amount: None,
            in_amount: *amount,
            out_amount: dst_lst_out,
            fee_mint: *output_mint,
            fee_amount,
            fee_pct,
        })
    }

    pub fn swap_exact_in(
        &self,
        SwapParams {
            in_amount,
            out_amount,
            source_mint,
            destination_mint,
            source_token_account,
            destination_token_account,
            token_transfer_authority,
            ..
        }: &SwapParams,
    ) -> anyhow::Result<SwapAndAccountMetas> {
        let (
            _,
            LstData {
                token_program: src_token_program,
                sol_val_calc: src_sol_val_calc,
                ..
            },
        ) = self.find_ready_lst(*source_mint)?;
        let (
            _,
            LstData {
                token_program: dst_token_program,
                sol_val_calc: dst_sol_val_calc,
                ..
            },
        ) = self.find_ready_lst(*destination_mint)?;
        let Instruction { accounts, .. } = swap_exact_in_ix_by_mint_full(
            SwapByMintsFreeArgs {
                signer: *token_transfer_authority,
                src_lst_acc: *source_token_account,
                dst_lst_acc: *destination_token_account,
                src_lst_mint: MintWithTokenProgram {
                    pubkey: *source_mint,
                    token_program: *src_token_program,
                },
                dst_lst_mint: MintWithTokenProgram {
                    pubkey: *destination_mint,
                    token_program: *dst_token_program,
                },
                lst_state_list: &self.lst_state_list_account,
            },
            SwapExactInAmounts {
                // TODO: where did other_amount_threshold go?
                min_amount_out: *out_amount,
                amount: *in_amount,
            },
            SrcDstLstSolValueCalcAccountSuffixes {
                src_lst_calculator_accounts: &src_sol_val_calc.ix_accounts(),
                dst_lst_calculator_accounts: &dst_sol_val_calc.ix_accounts(),
            },
            &self
                .pricing_prog()?
                .price_exact_in_accounts(PriceExactInKeys {
                    input_lst_mint: *source_mint,
                    output_lst_mint: *destination_mint,
                })?,
            self.pool_state()?.pricing_program,
        )?;
        Ok(SwapAndAccountMetas {
            // TODO: update this
            swap: jupiter_amm_interface::Swap::StakeDexStakeWrappedSol,
            account_metas: accounts,
        })
    }

    fn find_ready_lst(&self, lst_mint: Pubkey) -> anyhow::Result<(&LstState, &LstData)> {
        let (lst_state, lst_data) = self
            .lst_state_list()?
            .iter()
            .zip(self.lst_data_list.iter())
            .find(|(state, _data)| state.mint == lst_mint)
            .ok_or_else(|| anyhow!("LST {lst_mint} not on list"))?;
        let lst_data = lst_data
            .as_ref()
            .ok_or_else(|| anyhow!("LST {lst_mint} not supported"))?;
        Ok((lst_state, lst_data))
    }

    fn update_lst_state_list(&mut self, new_lst_state_list_account: Account) -> anyhow::Result<()> {
        // simple model for diffs:
        // - if new and old list differs in mints, then try to find the mismatches and replace them
        // - if sol val calc program changed, then just invalidate to None. Otherwise we would need a
        //   SanctumLstList to reinitialize the KnownLstSolValCalc
        // - if list was extended, the new entries will just be None and we cant handle it. Otherwise we would need a
        //   SanctumLstList to initialize the KnownLstSolValCalc
        let lst_state_list = self.lst_state_list()?;
        let new_lst_state_list = try_lst_state_list(&new_lst_state_list_account.data)?;
        if lst_state_list.len() == new_lst_state_list.len()
            && lst_state_list.iter().zip(new_lst_state_list.iter()).all(
                |(old_lst_state, new_lst_state)| {
                    old_lst_state.mint == new_lst_state.mint
                        && old_lst_state.sol_value_calculator == new_lst_state.sol_value_calculator
                },
            )
        {
            self.lst_state_list_account = new_lst_state_list_account;
            return Ok(());
        }
        // Either at least 1 sol value calculator changed or mint changed:
        // rebuild entire lst_data vec by cloning from old vec
        let mut new_lst_data_list = vec![None; new_lst_state_list.len()];
        lst_state_list
            .iter()
            .zip(self.lst_data_list.iter())
            .zip(new_lst_state_list.iter())
            .zip(new_lst_data_list.iter_mut())
            .for_each(
                |(((old_lst_state, old_lst_data), new_lst_state), new_lst_data)| {
                    let replacement = if old_lst_state.mint != new_lst_state.mint {
                        self.lst_data_list
                            .iter()
                            .find(|opt| match opt {
                                Some(ld) => {
                                    ld.sol_val_calc.lst_mint() == new_lst_state.mint
                                        && ld.sol_val_calc.sol_value_calculator_program_id()
                                            == new_lst_state.sol_value_calculator
                                }
                                None => false,
                            })
                            .cloned()
                            .flatten()
                    } else {
                        old_lst_data
                            .as_ref()
                            .map_or_else(
                                || None,
                                |ld| {
                                    if ld.sol_val_calc.sol_value_calculator_program_id()
                                        == new_lst_state.sol_value_calculator
                                    {
                                        Some(ld)
                                    } else {
                                        None
                                    }
                                },
                            )
                            .cloned()
                    };
                    *new_lst_data = replacement;
                },
            );
        self.lst_data_list = new_lst_data_list;
        self.lst_state_list_account = new_lst_state_list_account;
        Ok(())
    }
}

impl Amm for SPoolJup {
    /// Initialized by lst_state_list account, NOT pool_state.
    ///
    /// Params can optionally be a b58-encoded pubkey string that is the S controller program's program_id
    fn from_keyed_account(
        KeyedAccount {
            key,
            account,
            params,
        }: &KeyedAccount,
    ) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let (program_id, lst_state_list_addr) = match params {
            // default to INF if program-id params not provided
            None => (
                s_controller_lib::program::ID,
                s_controller_lib::program::LST_STATE_LIST_ID,
            ),
            Some(value) => {
                // TODO: maybe unnecessary clone() here?
                let program_id =
                    Pubkey::from_str(&serde_json::from_value::<String>(value.clone())?)?;
                (program_id, find_lst_state_list_address(program_id).0)
            }
        };
        if *key != lst_state_list_addr {
            return Err(anyhow!(
                "Incorrect LST state list addr. Expected {lst_state_list_addr}. Got {key}"
            ));
        }
        let SanctumLstList { sanctum_lst_list } = SanctumLstList::load();
        Self::from_lst_state_list_account(program_id, account.clone(), &sanctum_lst_list)
    }

    fn label(&self) -> String {
        LABEL.into()
    }

    fn program_id(&self) -> Pubkey {
        self.program_id
    }

    /// S Pools are 1 per program
    fn key(&self) -> Pubkey {
        self.program_id()
    }

    fn get_reserve_mints(&self) -> Vec<Pubkey> {
        let mut res: Vec<Pubkey> = match self.lst_state_list() {
            Ok(list) => list.iter().map(|LstState { mint, .. }| *mint).collect(),
            Err(_e) => vec![],
        };
        if let Ok(pool_state) = self.pool_state() {
            res.push(pool_state.lp_token_mint);
        }
        res
    }

    fn get_accounts_to_update(&self) -> Vec<Pubkey> {
        let mut res = vec![self.lst_state_list_addr, self.pool_state_addr];
        if let Some(pricing_prog) = &self.pricing_prog {
            res.extend(pricing_prog.get_accounts_to_update());
        }
        if let Ok(pool_state) = &self.pool_state() {
            res.push(pool_state.lp_token_mint);
        }
        if let Ok(lst_state_list) = self.lst_state_list() {
            res.extend(
                lst_state_list
                    .iter()
                    .zip(self.lst_data_list.iter())
                    .filter_map(|(lst_state, lst_data)| {
                        let lst_data = lst_data.as_ref()?;
                        let mut res = lst_data.sol_val_calc.get_accounts_to_update();
                        if let Ok(ata) = self.pool_reserves_account(lst_state, lst_data) {
                            res.push(ata);
                        }
                        Some(res)
                    })
                    .flatten(),
            );
        }
        res
    }

    fn update(&mut self, account_map: &AccountMap) -> anyhow::Result<()> {
        // returns the first encountered error, but tries to update everything eagerly
        // even after encountering an error

        // use raw indices to avoid lifetime errs from borrowing immut field (self.lst_state_list)
        // while borrowing mut field (self.lst_data_list)
        #[allow(clippy::manual_try_fold)] // we dont want to short-circuit, so dont try_fold()
        let mut res = (0..self.lst_data_list.len())
            .map(|i| {
                let ld = match &self.lst_data_list[i] {
                    Some(l) => l,
                    None => return Ok(()),
                };
                let ata_res = self.pool_reserves_account(&self.lst_state_list()?[i], ld);
                let ld = match &mut self.lst_data_list[i] {
                    Some(l) => l,
                    None => return Ok(()),
                };
                let r = ld.sol_val_calc.update(account_map);
                r.and(ata_res.map_or_else(
                    |e| Err(e.into()),
                    |ata| {
                        if let Some(fetched) = account_map.get(&ata) {
                            ld.reserves_balance = Some(token_account_balance(fetched)?);
                        }
                        Ok(())
                    },
                ))
            })
            .fold(Ok(()), |res, curr_res| res.and(curr_res));

        if let Some(pp) = self.pricing_prog.as_mut() {
            res = res.and(pp.update(account_map));
        }

        // update pool state and lst_state_list last so we can invalidate
        // pricing_prog and lst_sol_val_calcs if any of them changed

        // update lst_state_list first so we can use the new lst_state_list to reset pricing program
        if let Some(lst_state_list_acc) = account_map.get(&self.lst_state_list_addr) {
            res = res.and(self.update_lst_state_list(lst_state_list_acc.clone()));
        }

        if let Some(pool_state_acc) = account_map.get(&self.pool_state_addr) {
            res = res.and(try_pool_state(&pool_state_acc.data).map_or_else(
                |e| Err(e.into()),
                |ps| {
                    let lst_state_list = self.lst_state_list()?;
                    let mut r = Ok(());
                    // reinitialize pricing program if changed
                    if let Ok(old_ps) = self.pool_state() {
                        if old_ps.pricing_program != ps.pricing_program {
                            let new_pricing_prog = try_pricing_prog(ps, lst_state_list)
                                .map(|mut pp| {
                                    r = pp.update(account_map);
                                    pp
                                })
                                .ok();
                            self.pricing_prog = new_pricing_prog;
                        }
                    }
                    self.pool_state_account = Some(pool_state_acc.clone());
                    r
                },
            ));
        }

        // finally, update LP token supply after pool state has been updated
        if let Ok(pool_state) = self.pool_state() {
            if let Some(lp_token_mint_acc) = account_map.get(&pool_state.lp_token_mint) {
                match mint_supply(lp_token_mint_acc) {
                    Ok(supply) => self.lp_mint_supply = Some(supply),
                    Err(e) => res = res.and(Err(e.into())),
                }
            }
        }

        res
    }

    fn quote(&self, _quote_params: &QuoteParams) -> anyhow::Result<Quote> {
        todo!()
    }

    fn get_swap_and_account_metas(
        &self,
        _swap_params: &SwapParams,
    ) -> anyhow::Result<SwapAndAccountMetas> {
        todo!()
    }

    fn clone_amm(&self) -> Box<dyn Amm + Send + Sync> {
        Box::new(self.clone())
    }

    fn has_dynamic_accounts(&self) -> bool {
        true
    }

    /// TODO: this is not true for AddLiquidity and RemoveLiquidity
    fn supports_exact_out(&self) -> bool {
        true
    }
}

/// Returns (fee_amount, fee_pct)
/// fee_pct is [0.0, 1.0], not [0, 100],
/// so 0.1 (NOT 10.0) means 10%
fn calc_quote_fees(
    sol_value_amts: AmtsAfterFee,
    sol_val_calc: &KnownLstSolValCalc,
) -> anyhow::Result<(u64, Decimal)> {
    let fee_amount_sol = sol_value_amts.fee_charged();
    let fee_pct_num = Decimal::from_u64(fee_amount_sol)
        .ok_or_else(|| anyhow!("Decimal conv error fees_charged"))?;
    let fee_pct_denom = Decimal::from_u64(sol_value_amts.amt_before_fee()?)
        .ok_or_else(|| anyhow!("Decimal conv error amt_before_fee"))?;
    let fee_pct = fee_pct_num
        .checked_div(fee_pct_denom)
        .ok_or_else(|| anyhow!("Decimal fee_pct div err"))?;
    let fee_amount = sol_val_calc.sol_to_lst(fee_amount_sol)?.get_min();
    Ok((fee_amount, fee_pct))
}

/// Returns
/// (updated pool state, update lst state, reserves balance)
fn apply_sync_sol_value(
    mut pool_state: PoolState,
    mut lst_state: LstState,
    LstData {
        sol_val_calc,
        reserves_balance,
        token_program: _,
    }: &LstData,
) -> anyhow::Result<(PoolState, LstState, u64)> {
    let reserves_balance = *reserves_balance
        .as_ref()
        .ok_or_else(|| anyhow!("Reserves balance not fetched"))?;
    let ret_sol_val = sol_val_calc.lst_to_sol(reserves_balance)?;
    sync_sol_value_with_retval(&mut pool_state, &mut lst_state, ret_sol_val.get_min())?;
    Ok((pool_state, lst_state, reserves_balance))
}

fn try_pricing_prog(
    pool_state: &PoolState,
    lst_state_list: &[LstState],
) -> anyhow::Result<KnownPricingProg> {
    Ok(KnownPricingProg::try_new(
        pool_state.pricing_program,
        lst_state_list.iter().map(|LstState { mint, .. }| *mint),
    )?)
}

fn try_lst_data(
    lst_list: &[SanctumLst],
    LstState {
        mint,
        sol_value_calculator,
        ..
    }: &LstState,
) -> Option<LstData> {
    let SanctumLst {
        pool,
        token_program,
        ..
    } = lst_list.iter().find(|s| s.mint == *mint)?;
    let calc = match pool {
        PoolInfo::Lido => KnownLstSolValCalc::Lido(LidoLstSolValCalc::default()),
        PoolInfo::Marinade => KnownLstSolValCalc::Marinade(MarinadeLstSolValCalc::default()),
        PoolInfo::ReservePool => KnownLstSolValCalc::Wsol(WsolLstSolValCalc),
        PoolInfo::SanctumSpl(SplPoolAccounts { pool, .. }) => KnownLstSolValCalc::SanctumSpl(
            SanctumSplLstSolValCalc::from_keys(SplLstSolValCalcInitKeys {
                lst_mint: *mint,
                stake_pool_addr: *pool,
            }),
        ),
        PoolInfo::Spl(SplPoolAccounts { pool, .. }) => {
            KnownLstSolValCalc::Spl(SplLstSolValCalc::from_keys(SplLstSolValCalcInitKeys {
                lst_mint: *mint,
                stake_pool_addr: *pool,
            }))
        }
        PoolInfo::SPool(_) => None?,
    };
    if *sol_value_calculator != calc.sol_value_calculator_program_id() {
        None
    } else {
        Some(LstData {
            sol_val_calc: calc,
            reserves_balance: None,
            token_program: *token_program,
        })
    }
}
