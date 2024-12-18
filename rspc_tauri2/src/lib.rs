//! rspc-tauri2: [rspc](https://rspc.dev) adapter for [Tauri](https://tauri.app) v2.
#![cfg_attr(docsrs2, feature(doc_cfg))]
#![doc(
    html_logo_url = "https://github.com/oscartbeaumont/rspc/raw/main/docs/public/logo.png",
    html_favicon_url = "https://github.com/oscartbeaumont/rspc/raw/main/docs/public/logo.png"
)]

use std::{borrow::Borrow, collections::HashMap, sync::Arc};

use tauri::{
    plugin::{Builder, TauriPlugin},
    AppHandle, Manager, Runtime,
};
use tauri::Emitter;
use tauri::Listener;

use tokio::sync::{mpsc, Mutex};

use rspc::{
    internal::jsonrpc::{self, handle_json_rpc, Sender, SubscriptionMap},
    Router,
};
use serde_json::Value;

pub fn plugin<R: Runtime, TCtx, TMeta>(
    router: Arc<Router<TCtx, TMeta>>,
   // mut to_js_receiver: mpsc::UnboundedReceiver<String>,
    ctx_fn: impl Fn(AppHandle<R>) -> TCtx + Send + Sync + 'static,

) -> TauriPlugin<R>
where
    TCtx: Send + 'static,
    TMeta: Send + Sync + 'static,
{
    Builder::new("rspc")
        .setup(|app_handle, _| {
            let (tx, mut rx) = mpsc::unbounded_channel::<jsonrpc::Request>();
            let (resp_tx, mut resp_rx) = mpsc::unbounded_channel::<jsonrpc::Response>();
            // TODO: Don't keep using a tokio mutex. We don't need to hold it over the await point.
            let subscriptions = Arc::new(Mutex::new(HashMap::new()));
            //
            //
            // {
            //     let app_handle = app_handle.clone();
            //     let response_sender = resp_tx.clone();
            //     let subs = subscriptions.clone();
            //     {
            //         tauri::async_runtime::spawn(async move {
            //             while let Some(input) = to_js_receiver.recv().await {
            //
            //                 let map_guard = subs.lock().await;  // Lock the Mutex
            //                 let key = map_guard.keys().next().unwrap();
            //
            //                 let key = match key {
            //                     jsonrpc::RequestId::String(s) => s.clone(),
            //                     _ => "".to_string(),
            //                 };
            //
            //                 // Get the first key
            //                 // let sender = map_guard.get(key).unwrap();
            //                 println!("Sending to sender: {:?}", key);
            //                 // //sender(serde_json::Value::String(input.clone()))  // Call the function
            //
            //
            //                 let input = jsonrpc::ResponseInner::Event(serde_json::Value::String(input));
            //                 response_sender.send(jsonrpc::Response{
            //                     jsonrpc: "2.0",
            //                     id: jsonrpc::RequestId::String(key),
            //                     result: input,
            //                 });
            //             }
            //         });
            //     }
            // }

            tokio::spawn({
                let app_handle = app_handle.clone();
                async move {
                    while let Some(req) = rx.recv().await {
                        let ctx = ctx_fn(app_handle.clone());
                        let router = router.clone();
                        let mut resp_tx = resp_tx.clone();
                        let subscriptions = subscriptions.clone();
                        tokio::spawn(async move {
                            handle_json_rpc(
                                ctx,
                                req,
                                &router,
                                &mut Sender::ResponseChannel(&mut resp_tx),
                                &mut SubscriptionMap::Mutex(subscriptions.borrow()),
                            )
                            .await;
                        });
                    }
                }
            });

            {
                let app_handle = app_handle.clone();
                tokio::spawn(async move {
                    while let Some(event) = resp_rx.recv().await {
                        let _ = app_handle
                            .emit("plugin:rspc:transport:resp", event)
                            .map_err(|err| {
                                #[cfg(feature = "tracing")]
                                tracing::error!("failed to emit JSON-RPC response: {}", err);
                            });
                    }
                });
            }

            app_handle.listen_any("plugin:rspc:transport", move |event| {
                let _ = tx
                    .send(match serde_json::from_str(event.payload()) {
                        Ok(v) => v,
                        Err(err) => {
                            #[cfg(feature = "tracing")]
                            tracing::error!("failed to parse JSON-RPC request: {}", err);
                            return;
                        }
                    })
                    .map_err(|err| {
                        #[cfg(feature = "tracing")]
                        tracing::error!("failed to send JSON-RPC request: {}", err);
                    });
            });

            Ok(())
        })
        .build()
}
