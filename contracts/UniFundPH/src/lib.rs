#![no_std]

//! # unifund-dao
//!
//! A DAO smart contract tailored for "UniFund DAO", a decentralized treasury system for university student organizations.
//!
//! ## UniFund DAO Narrative Mapping
//! - **Admin** = "Faculty Advisor" (Has veto power over proposals).
//! - **Governance Token** = "Student Club Membership Token" (1 token = 1 vote weight).
//! - **Treasury Token** = "Club Funds" (e.g., USDC or XLM on testnet).
//! - **Proposals** = "Event Budgets" or "Equipment Purchases".
//!
//! ## Flow
//!
//! 1. **Deposit** — students or donors deposit Club Funds into the treasury.
//!
//! 2. **Propose** — any student with a Membership Token may submit a proposal (Event Budget).
//!
//! 3. **Vote** — students vote `For` or `Against`. Votes are weighted by Membership Tokens.
//!
//! 4. **Queue** — after voting, if quorum is reached and `For` > `Against`, the proposal is queued. Funds are immediately reserved.
//!
//! 5. **Veto** — the Faculty Advisor (Admin) may cancel the proposal during the veto period.
//!
//! 6. **Execute** — after the veto period expires, funds are transferred.
//!
//! 7. **Cancel** — Faculty Advisor may cancel any proposal that is not yet `Executed`.

use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, token, Address, Env, String, Symbol, Vec,
};

// ─────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────

const INITIALIZED: Symbol = symbol_short!("INIT");
const BPS_DENOM: u64 = 10_000;

// ─────────────────────────────────────────────
// Composite Key
// ─────────────────────────────────────────────

/// Key for a single vote on an Event Budget/Proposal.
#[contracttype]
#[derive(Clone)]
pub struct VoteKey {
    pub proposal_id: u64,
    pub voter: Address,
}

// ─────────────────────────────────────────────
// Storage Keys
// ─────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Global configuration.
    Config,
    /// Total unallocated Club Funds held in the treasury.
    TreasuryBalance,
    /// Total Club Funds reserved for queued Event Budgets.
    ReservedBalance,
    /// Monotonically-incrementing next proposal ID.
    NextProposalId,
    /// Full Event Budget proposal record keyed by ID.
    Proposal(u64),
    /// All Event Budget proposal IDs in global order.
    ProposalIndex,
    /// All Event Budget proposal IDs submitted by a student.
    ProposerProposals(Address),
    /// Vote weight cast by a specific student on a specific proposal.
    Vote(VoteKey),
}

// ─────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────

/// Global UniFund DAO configuration.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Config {
    /// Faculty Advisor — may veto Event Budgets, cancel any proposal, and update config.
    pub admin: Address,
    /// Club Funds (e.g. USDC/XLM) held in the treasury and used for payouts.
    pub treasury_token: Address,
    /// Student Club Membership Token whose balance determines voting weight.
    pub governance_token: Address,
    /// Minimum `for_votes / total_votes` in basis points (e.g. 5100 = 51%).
    pub quorum_bps: u32,
    /// Seconds the voting window stays open after an Event Budget is submitted.
    pub voting_window: u64,
    /// Seconds between queue and earliest execution (veto period for Faculty Advisor).
    pub veto_period: u64,
    /// Maximum amount any single Event Budget may request (0 = no cap).
    pub spending_cap: i128,
}

/// Lifecycle state of an Event Budget proposal.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum ProposalStatus {
    /// Voting window open for students.
    Active,
    /// Voting window closed; quorum not reached or against won.
    Defeated,
    /// Passed vote; inside veto window awaiting execution. Funds reserved.
    Queued,
    /// Executed; Club Funds transferred.
    Executed,
    /// Cancelled before execution (Faculty Advisor veto or proposer cancelled).
    Cancelled,
}

/// Direction of a student's vote.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum VoteDirection {
    For,
    Against,
    Abstain,
}

/// An Event Budget or Equipment Purchase Proposal.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Proposal {
    pub id: u64,
    pub proposer: Address, // The student proposing the budget
    pub title: String,
    pub description: String,
    /// Address that will receive the Club Funds if executed.
    pub recipient: Address,
    /// Requested Club Funds amount.
    pub amount: i128,
    pub status: ProposalStatus,
    /// Total Student Club Membership Token weight cast For.
    pub for_votes: u64,
    /// Total Student Club Membership Token weight cast Against.
    pub against_votes: u64,
    /// Total Student Club Membership Token weight cast Abstain.
    pub abstain_votes: u64,
    /// Timestamp after which votes are no longer accepted.
    pub voting_deadline: u64,
    /// Earliest timestamp at which `execute_proposal` may be called.
    pub executable_at: u64,
    pub submitted_at: u64,
    pub executed_at: u64,
}

// ─────────────────────────────────────────────
// Contract
// ─────────────────────────────────────────────

#[contract]
pub struct UniFundContract;

#[contractimpl]
impl UniFundContract {
    // ── Initialisation ───────────────────────

    /// Deploy the UniFund DAO treasury.
    pub fn initialize(
        env: Env,
        admin: Address, // Faculty Advisor
        treasury_token: Address, // Club Funds
        governance_token: Address, // Student Club Membership Token
        quorum_bps: u32,
        voting_window: u64,
        veto_period: u64,
        spending_cap: i128,
    ) {
        if env.storage().instance().has(&INITIALIZED) {
            panic!("already initialized");
        }
        assert!(quorum_bps > 0 && quorum_bps <= 10_000, "quorum_bps must be 1-10000");
        assert!(voting_window > 0, "voting window must be positive");
        assert!(veto_period > 0, "veto period must be positive");
        assert!(spending_cap >= 0, "spending cap cannot be negative");

        env.storage().instance().set(&INITIALIZED, &true);
        env.storage().instance().set(
            &DataKey::Config,
            &Config {
                admin,
                treasury_token,
                governance_token,
                quorum_bps,
                voting_window,
                veto_period,
                spending_cap,
            },
        );
        env.storage()
            .instance()
            .set(&DataKey::TreasuryBalance, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::ReservedBalance, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &1u64);
        env.storage()
            .instance()
            .set(&DataKey::ProposalIndex, &Vec::<u64>::new(&env));
    }

    // ── Admin (Faculty Advisor) ──────────────

    /// Update the quorum, voting window, veto period, or spending cap.
    /// Faculty Advisor only.
    pub fn update_config(
        env: Env,
        admin: Address,
        quorum_bps: u32,
        voting_window: u64,
        veto_period: u64,
        spending_cap: i128,
    ) {
        admin.require_auth();
        let mut config = Self::load_config(&env);
        assert!(config.admin == admin, "caller is not the Faculty Advisor (admin)");
        assert!(quorum_bps > 0 && quorum_bps <= 10_000, "quorum_bps must be 1-10000");
        assert!(voting_window > 0, "voting window must be positive");
        assert!(veto_period > 0, "veto period must be positive");
        assert!(spending_cap >= 0, "spending cap cannot be negative");
        config.quorum_bps = quorum_bps;
        config.voting_window = voting_window;
        config.veto_period = veto_period;
        config.spending_cap = spending_cap;
        env.storage().instance().set(&DataKey::Config, &config);
    }

    /// Transfer Faculty Advisor rights. Faculty Advisor only.
    pub fn transfer_admin(env: Env, admin: Address, new_admin: Address) {
        admin.require_auth();
        let mut config = Self::load_config(&env);
        assert!(config.admin == admin, "caller is not the Faculty Advisor (admin)");
        config.admin = new_admin;
        env.storage().instance().set(&DataKey::Config, &config);
    }

    // ── Treasury Funding ─────────────────────

    /// Deposit Club Funds into the UniFund treasury.
    ///
    /// Any student or donor may deposit. Tokens are held collectively for the club.
    pub fn deposit(env: Env, depositor: Address, amount: i128) {
        depositor.require_auth();
        assert!(amount > 0, "deposit amount must be positive");

        let config = Self::load_config(&env);
        token::Client::new(&env, &config.treasury_token).transfer(
            &depositor,
            &env.current_contract_address(),
            &amount,
        );

        let bal: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TreasuryBalance)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::TreasuryBalance, &(bal + amount));
    }

    // ── Proposals (Event Budgets) ────────────

    /// Submit an Event Budget proposal. Returns the proposal ID.
    ///
    /// Any student with a Membership Token may propose.
    pub fn submit_proposal(
        env: Env,
        proposer: Address,
        title: String,
        description: String,
        recipient: Address,
        amount: i128,
    ) -> u64 {
        proposer.require_auth();

        assert!(amount > 0, "amount must be positive");
        assert!(!title.is_empty(), "title cannot be empty");
        assert!(!description.is_empty(), "description cannot be empty");

        let config = Self::load_config(&env);

        // Proposer must hold at least 1 Student Club Membership Token
        let gov_bal = token::Client::new(&env, &config.governance_token)
            .balance(&proposer);
        assert!(gov_bal > 0, "proposer must hold a Student Club Membership Token");

        // Spending cap check
        if config.spending_cap > 0 {
            assert!(
                amount <= config.spending_cap,
                "amount exceeds event budget spending cap"
            );
        }

        // Available Club Funds check (treasury - already reserved)
        let treasury: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TreasuryBalance)
            .unwrap_or(0);
        let reserved: i128 = env
            .storage()
            .instance()
            .get(&DataKey::ReservedBalance)
            .unwrap_or(0);
        assert!(
            treasury - reserved >= amount,
            "insufficient available Club Funds"
        );

        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextProposalId)
            .unwrap();
        env.storage()
            .instance()
            .set(&DataKey::NextProposalId, &(id + 1));

        let now = env.ledger().timestamp();
        let proposal = Proposal {
            id,
            proposer: proposer.clone(),
            title,
            description,
            recipient,
            amount,
            status: ProposalStatus::Active,
            for_votes: 0,
            against_votes: 0,
            abstain_votes: 0,
            voting_deadline: now + config.voting_window,
            executable_at: 0,
            submitted_at: now,
            executed_at: 0,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Proposal(id), &proposal);

        // Global index
        let mut index: Vec<u64> = env
            .storage()
            .instance()
            .get(&DataKey::ProposalIndex)
            .unwrap_or(Vec::new(&env));
        index.push_back(id);
        env.storage()
            .instance()
            .set(&DataKey::ProposalIndex, &index);

        // Proposer index
        let mut plist: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::ProposerProposals(proposer.clone()))
            .unwrap_or(Vec::new(&env));
        plist.push_back(id);
        env.storage()
            .persistent()
            .set(&DataKey::ProposerProposals(proposer), &plist);

        id
    }

    // ── Voting ────────────────────────────────

    /// Cast a token-weighted vote on an Active Event Budget.
    ///
    /// Vote weight = caller's Membership Token balance at call time.
    pub fn vote(
        env: Env,
        voter: Address, // Student voter
        proposal_id: u64,
        direction: VoteDirection,
    ) {
        voter.require_auth();

        let mut proposal: Proposal = env
            .storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
            .expect("proposal not found");

        assert!(
            proposal.status == ProposalStatus::Active,
            "event budget is not active"
        );
        assert!(
            env.ledger().timestamp() <= proposal.voting_deadline,
            "voting window has closed"
        );

        let vote_key = DataKey::Vote(VoteKey {
            proposal_id,
            voter: voter.clone(),
        });
        assert!(
            !env.storage().persistent().has(&vote_key),
            "student has already voted on this event budget"
        );

        let config = Self::load_config(&env);
        let weight = token::Client::new(&env, &config.governance_token)
            .balance(&voter) as u64;
        assert!(weight > 0, "voter must hold a Student Club Membership Token");

        env.storage().persistent().set(&vote_key, &direction.clone());

        match direction {
            VoteDirection::For => proposal.for_votes += weight,
            VoteDirection::Against => proposal.against_votes += weight,
            VoteDirection::Abstain => proposal.abstain_votes += weight,
        }

        env.storage()
            .persistent()
            .set(&DataKey::Proposal(proposal_id), &proposal);
    }

    // ── Queue ─────────────────────────────────

    /// Queue an Event Budget that has passed its voting window.
    ///
    /// On success: reserves the Club Funds amount and starts the veto countdown for Faculty Advisor.
    pub fn queue_proposal(env: Env, proposal_id: u64) {
        let mut proposal: Proposal = env
            .storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
            .expect("proposal not found");

        assert!(
            proposal.status == ProposalStatus::Active,
            "event budget is not active"
        );
        assert!(
            env.ledger().timestamp() > proposal.voting_deadline,
            "voting window has not closed"
        );

        let config = Self::load_config(&env);
        let total = proposal.for_votes + proposal.against_votes + proposal.abstain_votes;

        let passed = proposal.for_votes > proposal.against_votes
            && (if total > 0 {
                proposal.for_votes * BPS_DENOM / total >= config.quorum_bps as u64
            } else {
                false
            });

        if passed {
            // Reserve Club Funds for this Event Budget immediately
            let reserved: i128 = env
                .storage()
                .instance()
                .get(&DataKey::ReservedBalance)
                .unwrap_or(0);
            env.storage()
                .instance()
                .set(&DataKey::ReservedBalance, &(reserved + proposal.amount));

            proposal.status = ProposalStatus::Queued;
            proposal.executable_at = env.ledger().timestamp() + config.veto_period;
        } else {
            proposal.status = ProposalStatus::Defeated;
        }

        env.storage()
            .persistent()
            .set(&DataKey::Proposal(proposal_id), &proposal);
    }

    // ── Veto / Cancel ─────────────────────────

    /// Cancel an Event Budget at any non-executed stage.
    ///
    /// - Faculty Advisor (Admin) may veto/cancel any non-executed budget.
    /// - The proposing student may cancel their own Active budget.
    pub fn cancel_proposal(env: Env, caller: Address, proposal_id: u64) {
        caller.require_auth();

        let config = Self::load_config(&env);
        let mut proposal: Proposal = env
            .storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
            .expect("proposal not found");

        assert!(
            proposal.status != ProposalStatus::Executed,
            "cannot cancel an executed event budget"
        );
        assert!(
            proposal.status != ProposalStatus::Cancelled,
            "event budget is already cancelled"
        );

        let is_admin = config.admin == caller;
        let is_proposer = proposal.proposer == caller;

        if is_proposer && !is_admin {
            assert!(
                proposal.status == ProposalStatus::Active,
                "student proposer may only cancel active event budgets"
            );
            assert!(
                env.ledger().timestamp() <= proposal.voting_deadline,
                "voting window has closed; only Faculty Advisor may cancel now"
            );
        } else {
            assert!(is_admin, "only Faculty Advisor or proposing student may cancel");
        }

        // Release reserved Club Funds if queued
        if proposal.status == ProposalStatus::Queued {
            let reserved: i128 = env
                .storage()
                .instance()
                .get(&DataKey::ReservedBalance)
                .unwrap_or(0);
            env.storage()
                .instance()
                .set(&DataKey::ReservedBalance, &(reserved - proposal.amount));
        }

        proposal.status = ProposalStatus::Cancelled;
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(proposal_id), &proposal);
    }

    // ── Execute ───────────────────────────────

    /// Execute a Queued Event Budget after the Faculty Advisor's veto period expires.
    /// Transfers Club Funds to `proposal.recipient`.
    pub fn execute_proposal(env: Env, proposal_id: u64) {
        let mut proposal: Proposal = env
            .storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
            .expect("proposal not found");

        assert!(
            proposal.status == ProposalStatus::Queued,
            "event budget is not queued"
        );

        let now = env.ledger().timestamp();
        assert!(
            now >= proposal.executable_at,
            "Faculty Advisor veto period has not expired"
        );

        let config = Self::load_config(&env);

        // Debit Club Funds and reserved balances
        let treasury: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TreasuryBalance)
            .unwrap_or(0);
        assert!(
            treasury >= proposal.amount,
            "treasury has insufficient Club Funds"
        );
        env.storage()
            .instance()
            .set(&DataKey::TreasuryBalance, &(treasury - proposal.amount));

        let reserved: i128 = env
            .storage()
            .instance()
            .get(&DataKey::ReservedBalance)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::ReservedBalance, &(reserved - proposal.amount));

        // Transfer to recipient
        token::Client::new(&env, &config.treasury_token).transfer(
            &env.current_contract_address(),
            &proposal.recipient,
            &proposal.amount,
        );

        proposal.status = ProposalStatus::Executed;
        proposal.executed_at = now;
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(proposal_id), &proposal);
    }

    // ── Queries ───────────────────────────────

    pub fn get_config(env: Env) -> Config {
        Self::load_config(&env)
    }

    pub fn get_treasury_balance(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::TreasuryBalance)
            .unwrap_or(0)
    }

    pub fn get_reserved_balance(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::ReservedBalance)
            .unwrap_or(0)
    }

    pub fn get_available_balance(env: Env) -> i128 {
        let treasury: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TreasuryBalance)
            .unwrap_or(0);
        let reserved: i128 = env
            .storage()
            .instance()
            .get(&DataKey::ReservedBalance)
            .unwrap_or(0);
        (treasury - reserved).max(0)
    }

    pub fn get_proposal(env: Env, proposal_id: u64) -> Proposal {
        env.storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
            .expect("proposal not found")
    }

    pub fn get_all_proposals(env: Env) -> Vec<u64> {
        env.storage()
            .instance()
            .get(&DataKey::ProposalIndex)
            .unwrap_or(Vec::new(&env))
    }

    pub fn get_proposer_proposals(env: Env, proposer: Address) -> Vec<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::ProposerProposals(proposer))
            .unwrap_or(Vec::new(&env))
    }

    pub fn get_vote(env: Env, proposal_id: u64, voter: Address) -> Option<VoteDirection> {
        env.storage()
            .persistent()
            .get(&DataKey::Vote(VoteKey { proposal_id, voter }))
    }

    pub fn has_voted(env: Env, proposal_id: u64, voter: Address) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::Vote(VoteKey { proposal_id, voter }))
    }

    // ── Internal ──────────────────────────────

    fn load_config(env: &Env) -> Config {
        env.storage().instance().get(&DataKey::Config).unwrap()
    }
}

#[cfg(test)]
mod test;
