#![allow(clippy::needless_return)]

// Built-in deps
use std::sync::Arc;
// External uses
use crate::eth_watch::EthWatchRequest;
use futures::channel::mpsc;
use jsonrpc_core::{MetaIoHandler, Result};
use jsonrpc_derive::rpc;
use jsonrpc_pubsub::{typed::Subscriber, PubSubHandler, Session, SubscriptionId};
use jsonrpc_ws_server::RequestContext;
use web3::types::Address;
// Workspace uses
use models::{
    config_options::{ConfigurationOptions, ThreadPanicNotify},
    node::tx::TxHash,
    ActionType, Operation,
};
use storage::ConnectionPool;
// Local uses
use crate::fee_ticker::TickerRequest;
use crate::{
    api_server::event_notify::{start_sub_notifier, EventNotifierRequest, EventSubscribeRequest},
    api_server::rpc_server::types::{ETHOpInfoResp, ResponseAccountState, TransactionInfoResp},
    mempool::MempoolRequest,
    signature_checker::VerifyTxSignatureRequest,
    state_keeper::{ExecutedOpsNotify, StateKeeperRequest},
    utils::current_zksync_info::CurrentZksyncInfo,
};

#[rpc]
pub trait RpcPubSub {
    type Metadata;

    #[pubsub(subscription = "tx", subscribe, name = "tx_subscribe", alias("tx_sub"))]
    fn subscribe_tx(
        &self,
        meta: Self::Metadata,
        subscriber: Subscriber<TransactionInfoResp>,
        hash: TxHash,
        action_type: ActionType,
    );
    #[pubsub(subscription = "tx", unsubscribe, name = "tx_unsubscribe")]
    fn unsubscribe_tx(
        &self,
        meta: Option<Self::Metadata>,
        subscription: SubscriptionId,
    ) -> Result<bool>;

    #[pubsub(
        subscription = "eth_op",
        subscribe,
        name = "ethop_subscribe",
        alias("ethop_sub")
    )]
    fn subscribe_ethop(
        &self,
        meta: Self::Metadata,
        subscriber: Subscriber<ETHOpInfoResp>,
        serial_id: u64,
        action_type: ActionType,
    );
    #[pubsub(subscription = "eth_op", unsubscribe, name = "ethop_unsubscribe")]
    fn unsubscribe_ethop(
        &self,
        meta: Option<Self::Metadata>,
        subscription: SubscriptionId,
    ) -> Result<bool>;

    #[pubsub(
        subscription = "account",
        subscribe,
        name = "account_subscribe",
        alias("account_sub")
    )]
    fn subscribe_account(
        &self,
        meta: Self::Metadata,
        subscriber: Subscriber<ResponseAccountState>,
        addr: Address,
        action_type: ActionType,
    );
    #[pubsub(subscription = "account", unsubscribe, name = "account_unsubscribe")]
    fn unsubscribe_account(
        &self,
        meta: Option<Self::Metadata>,
        subscription: SubscriptionId,
    ) -> Result<bool>;
}

impl RpcPubSub for RpcSubApp {
    type Metadata = Arc<Session>;

    // subscribe - sub id, sink
    // unsub - sub id

    fn subscribe_tx(
        &self,
        _meta: Self::Metadata,
        subscriber: Subscriber<TransactionInfoResp>,
        hash: TxHash,
        action: ActionType,
    ) {
        self.event_sub_sender
            .clone()
            .try_send(EventNotifierRequest::Sub(
                EventSubscribeRequest::Transaction {
                    hash,
                    action,
                    subscriber,
                },
            ))
            .unwrap_or_default();
    }
    fn unsubscribe_tx(&self, _meta: Option<Self::Metadata>, id: SubscriptionId) -> Result<bool> {
        self.event_sub_sender
            .clone()
            .try_send(EventNotifierRequest::Unsub(id))
            .unwrap_or_default();
        Ok(true)
    }

    fn subscribe_ethop(
        &self,
        _meta: Self::Metadata,
        subscriber: Subscriber<ETHOpInfoResp>,
        serial_id: u64,
        action: ActionType,
    ) {
        self.event_sub_sender
            .clone()
            .try_send(EventNotifierRequest::Sub(
                EventSubscribeRequest::PriorityOp {
                    serial_id,
                    action,
                    subscriber,
                },
            ))
            .unwrap_or_default();
    }
    fn unsubscribe_ethop(&self, _meta: Option<Self::Metadata>, id: SubscriptionId) -> Result<bool> {
        self.event_sub_sender
            .clone()
            .try_send(EventNotifierRequest::Unsub(id))
            .unwrap_or_default();
        Ok(true)
    }

    fn subscribe_account(
        &self,
        _meta: Self::Metadata,
        subscriber: Subscriber<ResponseAccountState>,
        address: Address,
        action: ActionType,
    ) {
        self.event_sub_sender
            .clone()
            .try_send(EventNotifierRequest::Sub(EventSubscribeRequest::Account {
                address,
                action,
                subscriber,
            }))
            .unwrap_or_default();
    }

    fn unsubscribe_account(
        &self,
        _meta: Option<Self::Metadata>,
        id: SubscriptionId,
    ) -> Result<bool> {
        self.event_sub_sender
            .clone()
            .try_send(EventNotifierRequest::Unsub(id))
            .unwrap_or_default();
        Ok(true)
    }
}

struct RpcSubApp {
    event_sub_sender: mpsc::Sender<EventNotifierRequest>,
}

#[allow(clippy::too_many_arguments)]
pub fn start_ws_server(
    config_options: &ConfigurationOptions,
    op_recv: mpsc::Receiver<Operation>,
    db_pool: ConnectionPool,
    mempool_request_sender: mpsc::Sender<MempoolRequest>,
    executed_tx_receiver: mpsc::Receiver<ExecutedOpsNotify>,
    state_keeper_request_sender: mpsc::Sender<StateKeeperRequest>,
    sign_verify_request_sender: mpsc::Sender<VerifyTxSignatureRequest>,
    eth_watcher_request_sender: mpsc::Sender<EthWatchRequest>,
    ticker_request_sender: mpsc::Sender<TickerRequest>,
    panic_notify: mpsc::Sender<bool>,
    each_cache_size: usize,
    current_zksync_info: CurrentZksyncInfo,
) {
    let config_options = config_options.clone();
    let addr = config_options.json_rpc_ws_server_address;

    let (event_sub_sender, event_sub_receiver) = mpsc::channel(2048);

    start_sub_notifier(
        db_pool.clone(),
        op_recv,
        event_sub_receiver,
        executed_tx_receiver,
        state_keeper_request_sender.clone(),
        each_cache_size,
    );

    std::thread::Builder::new()
        .name("json_rpc_ws".to_string())
        .spawn(move || {
            let _panic_sentinel = ThreadPanicNotify(panic_notify);

            let tokio_runtime = tokio::runtime::Builder::new()
                .threaded_scheduler()
                .enable_all()
                .build()
                .unwrap();

            let mut io = PubSubHandler::new(MetaIoHandler::default());

            let req_rpc_app = super::rpc_server::RpcApp::new(
                tokio_runtime.handle().clone(),
                &config_options,
                db_pool,
                mempool_request_sender,
                state_keeper_request_sender,
                sign_verify_request_sender,
                eth_watcher_request_sender,
                ticker_request_sender,
                current_zksync_info,
            );
            req_rpc_app.extend(&mut io);

            let rpc_sub_app = RpcSubApp { event_sub_sender };

            io.extend_with(rpc_sub_app.to_delegate());

            let task_executor = tokio_old::runtime::Builder::new()
                .name_prefix("ws-executor")
                .core_threads(100)
                .build()
                .expect("failed to build ws executor");

            let server = jsonrpc_ws_server::ServerBuilder::with_meta_extractor(
                io,
                |context: &RequestContext| Arc::new(Session::new(context.sender())),
            )
            .request_middleware(super::loggers::ws_rpc::request_middleware)
            .max_connections(1000)
            .event_loop_executor(task_executor.executor())
            .start(&addr)
            .expect("Unable to start RPC ws server");

            server.wait().expect("rpc ws server start");
        })
        .expect("JSON RPC ws thread");
}
