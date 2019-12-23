pub mod client;
pub mod prover_data;
pub mod server;

// Built-in deps
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::{fmt, thread, time};
// External deps
use bellman::groth16;
use ff::PrimeField;
use log::{error, info};
use pairing::bn256;
// Workspace deps

pub struct BabyProver<C: ApiClient> {
    circuit_params: groth16::Parameters<bn256::Bn256>,
    jubjub_params: franklin_crypto::alt_babyjubjub::AltJubjubBn256,
    api_client: C,
    heartbeat_interval: time::Duration,
    get_prover_data_timeout: time::Duration,
    stop_signal: Arc<AtomicBool>,
}

pub trait ApiClient {
    fn block_to_prove(&self) -> Result<Option<(i64, i32)>, failure::Error>;
    fn working_on(&self, job_id: i32) -> Result<(), failure::Error>;
    fn prover_data(
        &self,
        block: i64,
        timeout: time::Duration,
    ) -> Result<prover_data::ProverData, failure::Error>;
    fn publish(
        &self,
        block: i64,
        p: groth16::Proof<models::node::Engine>,
        public_data_commitment: models::node::Fr,
    ) -> Result<(), failure::Error>;
}

#[derive(Debug)]
pub enum BabyProverError {
    Api(String),
    Internal(String),
    Stop,
}

impl fmt::Display for BabyProverError {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        let desc = match self {
            BabyProverError::Api(s) => s,
            BabyProverError::Internal(s) => s,
            BabyProverError::Stop => "stop",
        };
        write!(f, "{}", desc)
    }
}

pub fn start<C: 'static + Sync + Send + ApiClient>(
    prover: BabyProver<C>,
    exit_err_tx: mpsc::Sender<BabyProverError>,
) {
    let (tx_block_start, rx_block_start) = mpsc::channel();
    let prover = Arc::new(prover);
    let prover_rc = Arc::clone(&prover);
    thread::spawn(move || {
        let tx_block_start2 = tx_block_start.clone();
        exit_err_tx
            .send(prover.run_rounds(tx_block_start))
            .expect("failed to send exit error");
        tx_block_start2
            .send((0, true))
            .expect("failed to send heartbeat exit request"); // exit heartbeat routine request.
    });
    prover_rc.keep_sending_work_heartbeats(rx_block_start);
}

impl<C: ApiClient> BabyProver<C> {
    pub fn new(
        circuit_params: groth16::Parameters<bn256::Bn256>,
        jubjub_params: franklin_crypto::alt_babyjubjub::AltJubjubBn256,
        api_client: C,
        heartbeat_interval: time::Duration,
        get_prover_data_timeout: time::Duration,
        stop_signal: Arc<AtomicBool>,
    ) -> Self {
        BabyProver {
            circuit_params,
            jubjub_params,
            api_client,
            heartbeat_interval,
            get_prover_data_timeout,
            stop_signal,
        }
    }

    fn run_rounds(&self, start_heartbeats_tx: mpsc::Sender<(i32, bool)>) -> BabyProverError {
        let mut rng = rand::OsRng::new().unwrap();
        let pause_duration = time::Duration::from_secs(models::node::config::PROVER_CYCLE_WAIT);

        info!("Running worker rounds");

        while !self.stop_signal.load(Ordering::SeqCst) {
            info!("Starting a next round");
            let ret = self.next_round(&mut rng, &start_heartbeats_tx);
            if let Err(err) = ret {
                match err {
                    BabyProverError::Api(text) => {
                        error!("could not reach api server: {}", text);
                    }
                    BabyProverError::Internal(_) => {
                        return err;
                    }
                    _ => {}
                };
            }
            info!("round completed.");
            thread::sleep(pause_duration);
        }
        BabyProverError::Stop
    }

    fn next_round(
        &self,
        rng: &mut rand::OsRng,
        start_heartbeats_tx: &mpsc::Sender<(i32, bool)>,
    ) -> Result<(), BabyProverError> {
        let block_to_prove = self.api_client.block_to_prove().map_err(|e| {
            let e = format!("failed to get block to prove {}", e);
            BabyProverError::Api(e)
        })?;

        let (block, job_id) = match block_to_prove {
            Some(b) => b,
            _ => {
                info!("no block to prove from the server");
                (0, 0)
            }
        };
        // Notify heartbeat routine on new proving block job or None.
        start_heartbeats_tx
            .send((job_id, false))
            .expect("failed to send new job to heartbeat routine");
        if job_id == 0 {
            return Ok(());
        }
        let prover_data = self
            .api_client
            .prover_data(block, self.get_prover_data_timeout)
            .map_err(|err| {
                BabyProverError::Api(format!(
                    "could not get prover data for block {}: {}",
                    block, err
                ))
            })?;
        info!("starting to compute proof for block {}", block);

        let instance = circuit::circuit::FranklinCircuit {
            params: &self.jubjub_params,
            operation_batch_size: models::params::block_size_chunks(),
            old_root: Some(prover_data.old_root),
            new_root: Some(prover_data.new_root),
            block_number: models::node::Fr::from_str(&(block).to_string()),
            validator_address: Some(prover_data.validator_address),
            pub_data_commitment: Some(prover_data.public_data_commitment),
            operations: prover_data.operations,
            validator_balances: prover_data.validator_balances,
            validator_audit_path: prover_data.validator_audit_path,
            validator_account: prover_data.validator_account,
        };

        let proof = bellman::groth16::create_random_proof(instance, &self.circuit_params, rng);

        if let Err(e) = proof {
            return Err(BabyProverError::Internal(format!(
                "failed to create a proof: {}",
                e
            )));
        }

        // TODO: handle error.
        let p = proof.unwrap();

        let pvk = bellman::groth16::prepare_verifying_key(&self.circuit_params.vk);

        let res =
            bellman::groth16::verify_proof(&pvk, &p.clone(), &[prover_data.public_data_commitment]);
        if let Err(e) = res {
            return Err(BabyProverError::Internal(format!(
                "failed to verify created proof: {}",
                e
            )));
        }
        if !res.unwrap() {
            return Err(BabyProverError::Internal(
                "created proof did not pass verification".to_owned(),
            ));
        }

        let ret = self
            .api_client
            .publish(block, p, prover_data.public_data_commitment);
        if let Err(e) = ret {
            return Err(BabyProverError::Api(format!(
                "failed to publish proof: {}",
                e
            )));
        }

        info!("finished and published proof for block {}", block);

        Ok(())
    }

    fn keep_sending_work_heartbeats(&self, start_heartbeats_rx: mpsc::Receiver<(i32, bool)>) {
        let mut job_id = 0;
        while !self.stop_signal.load(Ordering::SeqCst) {
            thread::sleep(self.heartbeat_interval);
            let (j, quit) = match start_heartbeats_rx.try_recv() {
                Ok(v) => v,
                Err(mpsc::TryRecvError::Empty) => (job_id, false),
                Err(e) => {
                    panic!("error receiving from hearbeat channel: {}", e);
                }
            };
            if quit {
                return;
            }
            job_id = j;
            if job_id != 0 {
                info!("sending working_on request for job_id: {}", job_id);
                let ret = self.api_client.working_on(job_id);
                if let Err(e) = ret {
                    error!("working_on request errored: {}", e);
                }
            }
        }
    }
}
