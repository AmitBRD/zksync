// Built-in deps
use std::str::FromStr;
use std::{net, thread, time};
// External deps
use ff::{Field, PrimeField};
// Workspace deps
use prover::ApiClient;
use prover::{client, server};
use testhelper::TestAccount;

fn spawn_server(prover_timeout: time::Duration, rounds_interval: time::Duration) -> String {
    // TODO: make single server spawn for all tests
    let bind_to = "127.0.0.1:8088";
    let addr = net::SocketAddr::from_str(bind_to).unwrap();
    thread::spawn(move || {
        server::start_server(&addr, prover_timeout, rounds_interval);
    });
    bind_to.to_string()
}

fn access_storage() -> storage::StorageProcessor {
    storage::ConnectionPool::new()
        .access_storage()
        .expect("failed to connect to db")
}

#[test]
#[should_panic]
fn client_with_empty_worker_name_panics() {
    client::ApiClient::new("", "");
}

#[test]
fn api_client_register_start_and_stop_of_prover() {
    let addr = spawn_server(time::Duration::from_secs(1), time::Duration::from_secs(1));
    let client = client::ApiClient::new(&format!("http://{}", &addr), "foo");
    let id = client.register_prover().expect("failed to register");
    let storage = access_storage();
    storage
        .prover_by_id(id)
        .expect("failed to select registered prover");
    client.prover_stopped(id).expect("unexpected error");
    let prover = storage
        .prover_by_id(id)
        .expect("failed to select registered prover");
    prover.stopped_at.expect("expected not empty");
}

#[test]
fn api_client_simple_simulation() {
    let prover_timeout = time::Duration::from_secs(1);
    let rounds_interval = time::Duration::from_secs(10);

    let addr = spawn_server(prover_timeout, rounds_interval);

    let client = client::ApiClient::new(&format!("http://{}", &addr), "foo");

    // call block_to_prove and check its none
    let to_prove = client
        .block_to_prove()
        .expect("failed to get block to prove");
    assert!(to_prove.is_none());

    let storage = access_storage();

    let (op, wanted_prover_data) = test_operation_and_wanted_prover_data();

    println!("inserting test operation");
    // write test commit operation to db
    storage
        .execute_operation(&op)
        .expect("failed to mock commit operation");

    thread::sleep(time::Duration::from_secs(10));

    // should return block
    let to_prove = client
        .block_to_prove()
        .expect("failed to bet block to prove");
    assert!(to_prove.is_some());

    // block is taken unless no heartbeat from prover within prover_timeout period
    // should return None at this moment
    let to_prove = client
        .block_to_prove()
        .expect("failed to get block to prove");
    assert!(to_prove.is_none());

    // make block available
    thread::sleep(prover_timeout * 10);

    let to_prove = client
        .block_to_prove()
        .expect("failed to get block to prove");
    assert!(to_prove.is_some());

    let (block, job) = to_prove.unwrap();
    // sleep for prover_timeout and send heartbeat
    thread::sleep(prover_timeout * 2);
    client.working_on(job).unwrap();

    let to_prove = client
        .block_to_prove()
        .expect("failed to get block to prove");
    assert!(to_prove.is_none());

    let prover_data = client
        .prover_data(block, time::Duration::from_secs(30 * 60))
        .expect("failed to get prover data");
    assert_eq!(prover_data.old_root, wanted_prover_data.old_root);
    assert_eq!(prover_data.new_root, wanted_prover_data.new_root);
    assert_eq!(
        prover_data.public_data_commitment,
        wanted_prover_data.public_data_commitment,
    );
}

pub fn test_operation_and_wanted_prover_data(
) -> (models::Operation, prover::prover_data::ProverData) {
    let mut circuit_tree =
        models::circuit::CircuitAccountTree::new(models::params::account_tree_depth() as u32);
    // insert account and its balance
    let storage = access_storage();

    let validator_test_account = TestAccount::new();

    // Fee account
    let mut accounts = models::node::AccountMap::default();
    let mut validator_account = models::node::Account::default();
    validator_account.address = validator_test_account.address.clone();
    let validator_account_id: u32 = 0;
    accounts.insert(validator_account_id, validator_account.clone());

    let mut state = plasma::state::PlasmaState::new(accounts, 1);
    println!(
        "acc_number {}, acc {:?}",
        0,
        models::circuit::account::CircuitAccount::from(validator_account.clone()).pub_key_hash,
    );
    circuit_tree.insert(
        0,
        models::circuit::account::CircuitAccount::from(validator_account.clone()),
    );
    let initial_root = circuit_tree.root_hash();
    let deposit_priority_op = models::node::FranklinPriorityOp::Deposit(models::node::Deposit {
        sender: web3::types::Address::zero(),
        token: 0,
        amount: bigdecimal::BigDecimal::from(10),
        account: validator_test_account.address.clone(),
    });
    let mut op_success = state.execute_priority_op(deposit_priority_op.clone());
    let mut fees = Vec::new();
    let mut ops = Vec::new();
    let mut accounts_updated = Vec::new();

    if let Some(fee) = op_success.fee {
        fees.push(fee);
    }

    accounts_updated.append(&mut op_success.updates);

    storage
        .commit_state_update(
            0,
            &[(
                0,
                models::node::AccountUpdate::Create {
                    address: validator_account.address,
                    nonce: validator_account.nonce,
                },
            )],
        )
        .unwrap();
    storage.apply_state_update(0).unwrap();

    ops.push(models::node::ExecutedOperations::PriorityOp(Box::new(
        models::node::ExecutedPriorityOp {
            op: op_success.executed_op,
            priority_op: models::node::PriorityOp {
                serial_id: 0,
                data: deposit_priority_op.clone(),
                deadline_block: 2,
                eth_fee: bigdecimal::BigDecimal::from(0),
                eth_hash: vec![0; 8],
            },
            block_index: 0,
        },
    )));

    let (fee_account_id, fee_updates) = state.collect_fee(&fees, &validator_test_account.address);
    accounts_updated.extend(fee_updates.into_iter());

    let block = models::node::block::Block {
        block_number: state.block_number,
        new_root_hash: state.root_hash(),
        fee_account: fee_account_id,
        block_transactions: ops,
        processed_priority_ops: (0, 1),
    };

    let mut pub_data = vec![];
    let mut operations = vec![];

    if let models::node::FranklinPriorityOp::Deposit(deposit_op) = deposit_priority_op {
        let deposit_witness = circuit::witness::deposit::apply_deposit_tx(
            &mut circuit_tree,
            &models::node::operations::DepositOp {
                priority_op: deposit_op,
                account_id: 0,
            },
        );

        let deposit_operations =
            circuit::witness::deposit::calculate_deposit_operations_from_witness(
                &deposit_witness,
                &models::node::Fr::zero(),
                &models::node::Fr::zero(),
                &models::node::Fr::zero(),
                &circuit::operation::SignatureData {
                    r_packed: vec![Some(false); 256],
                    s: vec![Some(false); 256],
                },
                &[Some(false); 256],
            );
        operations.extend(deposit_operations);
        pub_data.extend(deposit_witness.get_pubdata());
    }

    let phaser = models::merkle_tree::PedersenHasher::<models::node::Engine>::default();
    let jubjub_params = &franklin_crypto::alt_babyjubjub::AltJubjubBn256::new();
    for _ in 0..models::params::block_size_chunks() - operations.len() {
        let (signature, first_sig_msg, second_sig_msg, third_sig_msg, _a, _b) =
            circuit::witness::utils::generate_dummy_sig_data(&[false], &phaser, &jubjub_params);

        operations.push(circuit::witness::noop::noop_operation(
            &circuit_tree,
            block.fee_account,
            &first_sig_msg,
            &second_sig_msg,
            &third_sig_msg,
            &signature,
            &[Some(false); 256],
        ));
        pub_data.extend(vec![false; 64]);
    }
    assert_eq!(pub_data.len(), 64 * models::params::block_size_chunks());
    assert_eq!(operations.len(), models::params::block_size_chunks());

    let validator_acc = circuit_tree
        .get(block.fee_account as u32)
        .expect("fee_account is not empty");
    let mut validator_balances = vec![];
    for i in 0..1 << models::params::BALANCE_TREE_DEPTH {
        let balance_value = match validator_acc.subtree.get(i as u32) {
            None => models::node::Fr::zero(),
            Some(bal) => bal.value,
        };
        validator_balances.push(Some(balance_value));
    }
    let _: models::node::Fr = circuit_tree.root_hash();
    let (root_after_fee, validator_account_witness) =
        circuit::witness::utils::apply_fee(&mut circuit_tree, block.fee_account, 0, 0);

    assert_eq!(root_after_fee, block.new_root_hash);
    let (validator_audit_path, _) =
        circuit::witness::utils::get_audits(&circuit_tree, block.fee_account as u32, 0);
    let public_data_commitment =
        circuit::witness::utils::public_data_commitment::<models::node::Engine>(
            &pub_data,
            Some(initial_root),
            Some(root_after_fee),
            Some(models::node::Fr::from_str(&block.fee_account.to_string()).unwrap()),
            Some(models::node::Fr::from_str(&(block.block_number).to_string()).unwrap()),
        );

    (
        models::Operation {
            id: None,
            action: models::Action::Commit,
            block: block.clone(),
            accounts_updated,
        },
        prover::prover_data::ProverData {
            public_data_commitment,
            old_root: initial_root,
            new_root: block.new_root_hash,
            validator_address: models::node::Fr::from_str(&block.fee_account.to_string()).unwrap(),
            operations,
            validator_balances,
            validator_audit_path,
            validator_account: validator_account_witness,
        },
    )
}

#[test]
fn api_server_publish_dummy() {
    let prover_timeout = time::Duration::from_secs(1);
    let rounds_interval = time::Duration::from_secs(10);
    let addr = spawn_server(prover_timeout, rounds_interval);

    let client = reqwest::Client::new();
    let res = client
        .post(&format!("http://{}/publish", &addr))
        .json(&server::PublishReq {
            block: 1,
            proof: models::EncodedProof::default(),
        })
        .send()
        .expect("failed to send publish request");

    assert_eq!(res.status(), reqwest::StatusCode::OK);
}
