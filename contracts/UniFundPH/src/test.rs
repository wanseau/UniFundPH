#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env, String,
};

fn create_token_contract<'a>(env: &Env, admin: &Address) -> token::StellarAssetClient<'a> {
    let contract_address = env.register_stellar_asset_contract(admin.clone());
    token::StellarAssetClient::new(env, &contract_address)
}

fn setup_env<'a>() -> (
    Env,
    UniFundContractClient<'a>,
    Address,
    token::StellarAssetClient<'a>,
    token::StellarAssetClient<'a>,
) {
    let env = Env::default();
    env.mock_all_auths();
    
    let contract_id = env.register_contract(None, UniFundContract);
    let client = UniFundContractClient::new(&env, &contract_id);
    
    let admin = Address::generate(&env);
    
    let treasury_token = create_token_contract(&env, &admin);
    let governance_token = create_token_contract(&env, &admin);
    
    (env, client, admin, treasury_token, governance_token)
}

#[test]
fn test_successful_club_budget_proposal() {
    let (env, client, admin, treasury_token, gov_token) = setup_env();
    let proposer = Address::generate(&env);
    let voter1 = Address::generate(&env);
    let voter2 = Address::generate(&env);
    let recipient = Address::generate(&env);
    
    // Config: Quorum/approval: 50%, Voting window: 100s, Veto period: 50s, Cap: 1000
    client.initialize(
        &admin,
        &treasury_token.address,
        &gov_token.address,
        &5000,
        &100,
        &50,
        &1000,
    );
    
    gov_token.mint(&proposer, &100);
    gov_token.mint(&voter1, &500);
    gov_token.mint(&voter2, &400);
    
    let donor = Address::generate(&env);
    treasury_token.mint(&donor, &5000);
    client.deposit(&donor, &5000);
    assert_eq!(client.get_treasury_balance(), 5000);
    
    let title = String::from_str(&env, "Annual Party");
    let desc = String::from_str(&env, "Budget for food and drinks");
    
    let proposal_id = client.submit_proposal(&proposer, &title, &desc, &recipient, &500);
    
    client.vote(&voter1, &proposal_id, &VoteDirection::For); // 500 votes for
    client.vote(&voter2, &proposal_id, &VoteDirection::Abstain); // 400 votes abstain
    // for_votes (500) / total (900) = 55.5% > 50%
    
    // Advance time past voting window
    env.ledger().with_mut(|li| {
        li.timestamp += 101;
    });
    
    client.queue_proposal(&proposal_id);
    
    assert_eq!(client.get_treasury_balance(), 5000);
    assert_eq!(client.get_reserved_balance(), 500);
    assert_eq!(client.get_available_balance(), 4500);
    
    let prop = client.get_proposal(&proposal_id);
    assert_eq!(prop.status, ProposalStatus::Queued);
    
    // Advance time past veto period
    env.ledger().with_mut(|li| {
        li.timestamp += 51;
    });
    
    client.execute_proposal(&proposal_id);
    
    assert_eq!(client.get_treasury_balance(), 4500);
    assert_eq!(client.get_reserved_balance(), 0);
    assert_eq!(treasury_token.balance(&recipient), 500);
    
    let prop_exec = client.get_proposal(&proposal_id);
    assert_eq!(prop_exec.status, ProposalStatus::Executed);
}

#[test]
fn test_faculty_advisor_veto() {
    let (env, client, admin, treasury_token, gov_token) = setup_env();
    let proposer = Address::generate(&env);
    let voter = Address::generate(&env);
    let recipient = Address::generate(&env);
    
    client.initialize(
        &admin,
        &treasury_token.address,
        &gov_token.address,
        &5000,
        &100,
        &50,
        &1000,
    );
    
    gov_token.mint(&proposer, &100);
    gov_token.mint(&voter, &1000);
    
    let donor = Address::generate(&env);
    treasury_token.mint(&donor, &5000);
    client.deposit(&donor, &5000);
    
    let title = String::from_str(&env, "Sketchy purchase");
    let desc = String::from_str(&env, "Buying a boat");
    
    let proposal_id = client.submit_proposal(&proposer, &title, &desc, &recipient, &900);
    
    client.vote(&voter, &proposal_id, &VoteDirection::For);
    
    // Advance time past voting window
    env.ledger().with_mut(|li| {
        li.timestamp += 101;
    });
    
    client.queue_proposal(&proposal_id);
    assert_eq!(client.get_reserved_balance(), 900);
    
    // Faculty advisor vetoes before veto period ends
    client.cancel_proposal(&admin, &proposal_id);
    
    let prop = client.get_proposal(&proposal_id);
    assert_eq!(prop.status, ProposalStatus::Cancelled);
    
    // Reserved balance should be cleared
    assert_eq!(client.get_reserved_balance(), 0);
    assert_eq!(client.get_available_balance(), 5000);
}

#[test]
fn test_failed_quorum_for_event() {
    let (env, client, admin, treasury_token, gov_token) = setup_env();
    let proposer = Address::generate(&env);
    let voter1 = Address::generate(&env);
    let voter2 = Address::generate(&env);
    let recipient = Address::generate(&env);
    
    // Quorum/Approval: 60%
    client.initialize(
        &admin,
        &treasury_token.address,
        &gov_token.address,
        &6000,
        &100,
        &50,
        &1000,
    );
    
    gov_token.mint(&proposer, &100);
    gov_token.mint(&voter1, &400); // 400 votes
    gov_token.mint(&voter2, &600); // 600 votes
    
    let donor = Address::generate(&env);
    treasury_token.mint(&donor, &5000);
    client.deposit(&donor, &5000);
    
    let title = String::from_str(&env, "Underfunded Event");
    let desc = String::from_str(&env, "Not enough support");
    
    let proposal_id = client.submit_proposal(&proposer, &title, &desc, &recipient, &500);
    
    // Total cast: 1000. For votes: 400 (40%), which is less than 60%.
    client.vote(&voter1, &proposal_id, &VoteDirection::For);
    client.vote(&voter2, &proposal_id, &VoteDirection::Abstain);
    
    // Advance time past voting window
    env.ledger().with_mut(|li| {
        li.timestamp += 101;
    });
    
    // Attempt to queue
    client.queue_proposal(&proposal_id);
    
    let prop = client.get_proposal(&proposal_id);
    
    // Since approval is only 40% < 60%, the proposal should be Defeated
    assert_eq!(prop.status, ProposalStatus::Defeated);
    
    // Reserved balance should remain 0
    assert_eq!(client.get_reserved_balance(), 0);
    assert_eq!(client.get_available_balance(), 5000);
}
