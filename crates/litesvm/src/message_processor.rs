// copied from agave commit 8340ee60dd0184b17cfd319ea504079dfa636b62
// with some execute_timings usage removed
use {
    solana_measure::measure_us,
    solana_program_runtime::invoke_context::InvokeContext,
    solana_svm_transaction::svm_message::SVMMessage,
    solana_timings::{ExecuteDetailsTimings, ExecuteTimings},
    solana_transaction_context::{IndexOfAccount, InstructionAccount},
    solana_transaction_error::TransactionError,
};

/// Process a message.
/// This method calls each instruction in the message over the set of loaded accounts.
/// For each instruction it calls the program entrypoint method and verifies that the result of
/// the call does not violate the bank's accounting rules.
/// The accounts are committed back to the bank only if every instruction succeeds.
pub(crate) fn process_message(
    message: &impl SVMMessage,
    program_indices: &[Vec<IndexOfAccount>],
    invoke_context: &mut InvokeContext,
    execute_timings: &mut ExecuteTimings,
    accumulated_consumed_units: &mut u64,
) -> Result<(), TransactionError> {
    debug_assert_eq!(program_indices.len(), message.num_instructions());
    for (top_level_instruction_index, ((program_id, instruction), program_indices)) in message
        .program_instructions_iter()
        .zip(program_indices.iter())
        .enumerate()
    {
        let mut instruction_accounts = Vec::with_capacity(instruction.accounts.len());
        for (instruction_account_index, index_in_transaction) in
            instruction.accounts.iter().enumerate()
        {
            let index_in_callee = instruction
                .accounts
                .get(0..instruction_account_index)
                .ok_or(TransactionError::InvalidAccountIndex)?
                .iter()
                .position(|account_index| account_index == index_in_transaction)
                .unwrap_or(instruction_account_index)
                as IndexOfAccount;
            let index_in_transaction = *index_in_transaction as usize;
            instruction_accounts.push(InstructionAccount {
                index_in_transaction: index_in_transaction as IndexOfAccount,
                index_in_caller: index_in_transaction as IndexOfAccount,
                index_in_callee,
                is_signer: message.is_signer(index_in_transaction),
                is_writable: message.is_writable(index_in_transaction),
            });
        }

        let mut compute_units_consumed = 0;
        let (result, process_instruction_us) = measure_us!({
            if invoke_context.is_precompile(program_id) {
                invoke_context.process_precompile(
                    program_id,
                    instruction.data,
                    &instruction_accounts,
                    program_indices,
                    message.instructions_iter().map(|ix| ix.data),
                )
            } else {
                invoke_context.process_instruction(
                    instruction.data,
                    &instruction_accounts,
                    program_indices,
                    &mut compute_units_consumed,
                    execute_timings,
                )
            }
        });

        *accumulated_consumed_units =
            accumulated_consumed_units.saturating_add(compute_units_consumed);
        // The per_program_timings are only used for metrics reporting at the trace
        // level, so they should only be accumulated when trace level is enabled.
        if log::log_enabled!(log::Level::Trace) {
            execute_timings.details.accumulate_program(
                program_id,
                process_instruction_us,
                compute_units_consumed,
                result.is_err(),
            );
        }
        invoke_context.timings = {
            execute_timings.details.accumulate(&invoke_context.timings);
            ExecuteDetailsTimings::default()
        };
        execute_timings
            .execute_accessories
            .process_instructions
            .total_us += process_instruction_us;

        result.map_err(|err| {
            TransactionError::InstructionError(top_level_instruction_index as u8, err)
        })?;
    }
    Ok(())
}
