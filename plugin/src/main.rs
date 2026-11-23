use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, BufRead, Write},
    sync::{Arc, Mutex},
    thread,
};
use wu_plus_subversion_provider::{Cancellation, RpcError, handle};

const MAX_REQUEST: usize = 16 * 1024 * 1024;
const MAX_PENDING: usize = 16;

fn send(response: Value) {
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "{response}");
}

fn respond(id: u64, result: Result<Value, RpcError>) {
    match result {
        Ok(result) => send(json!({"jsonrpc":"2.0","id":id,"result":result})),
        Err(error) => send(
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":error.message,"data":{"kind":error.kind,"exit_code":error.exit_code}}}),
        ),
    }
}

fn main() {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let interactive_result = match arguments.as_slice() {
        [operation, url] if operation == "--verify-certificate" => {
            Some(wu_plus_subversion_provider::verify_certificate(url))
        }
        [operation, root, path] if operation == "--resolve-interactively" => Some(
            wu_plus_subversion_provider::resolve_interactively(std::path::Path::new(root), path),
        ),
        [] => None,
        _ => Some(Err(RpcError::new(
            "invalid_request",
            "Unknown SVN provider command.",
        ))),
    };
    if let Some(result) = interactive_result {
        let code = result.unwrap_or_else(|error| {
            eprintln!("{}", error.message);
            1
        });
        std::process::exit(code);
    }
    let pending = Arc::new(Mutex::new(HashMap::<u64, Cancellation>::new()));
    let mut workers = Vec::new();
    for input in io::stdin().lock().lines() {
        let Ok(line) = input else { break };
        if line.len() > MAX_REQUEST {
            break;
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            send(
                json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Invalid JSON."}}),
            );
            continue;
        };
        let id = request["id"].as_u64();
        let method = request["method"].as_str().unwrap_or_default();
        let mut active = pending.lock().expect("pending requests");
        if method == "$/cancelRequest" {
            if let Some(id) = request["params"]["id"].as_u64()
                && let Some(request) = active.get(&id)
            {
                request.cancel();
            }
            continue;
        }
        let Some(id) = id else { continue };
        if request["jsonrpc"] != "2.0" || method.is_empty() {
            respond(
                id,
                Err(RpcError::new(
                    "invalid_request",
                    "Expected a JSON-RPC 2.0 request.",
                )),
            );
            continue;
        }
        if active.contains_key(&id) || active.len() >= MAX_PENDING {
            respond(
                id,
                Err(RpcError::new("busy", "Too many pending SVN requests.")),
            );
            continue;
        }
        let cancellation = Cancellation::default();
        active.insert(id, cancellation.clone());
        drop(active);
        let pending = Arc::clone(&pending);
        workers.retain(|worker: &thread::JoinHandle<()>| !worker.is_finished());
        workers.push(thread::spawn(move || {
            let method = request["method"].as_str().expect("validated method");
            let result = handle(method, request["params"].clone(), &cancellation);
            pending.lock().expect("pending requests").remove(&id);
            respond(id, result);
        }));
    }
    for request in pending.lock().expect("pending requests").values() {
        request.cancel();
    }
    for worker in workers {
        let _ = worker.join();
    }
}
