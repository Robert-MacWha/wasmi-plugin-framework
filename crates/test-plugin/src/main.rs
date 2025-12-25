use rand::Rng;
use serde_json::{self, Value};
use std::io::stderr;
use tracing::{error, info, level_filters::LevelFilter};
use tracing_subscriber::fmt;
use wasmi_plugin_pdk::{rpc_message::RpcError, server::PluginServer, transport::Transport};

async fn ping(_: Transport, _: ()) -> Result<Value, RpcError> {
    Ok(Value::String("pong".to_string()))
}

async fn get_random_number(_: Transport, _: ()) -> Result<Value, RpcError> {
    let mut rng = rand::rng();
    let random_number: u64 = rng.random();
    Ok(Value::Number(random_number.into()))
}

async fn get_time(_: Transport, _: ()) -> Result<Value, RpcError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| {
            error!("System time before UNIX EPOCH: {}", e);
            RpcError::InternalError
        })?;
    Ok(Value::Number(now.as_secs().into()))
}

async fn sleep(_: Transport, duration_ms: u64) -> Result<(), RpcError> {
    info!("Sleeping for {} milliseconds", duration_ms);
    std::thread::sleep(std::time::Duration::from_millis(duration_ms));
    info!("Woke up after sleeping for {} milliseconds", duration_ms);
    Ok(())
}

async fn call(transport: Transport, _: ()) -> Result<Value, RpcError> {
    let resp = transport.call("ping", Value::Null)?;
    Ok(resp.result)
}

async fn call_many(transport: Transport, limit: u64) -> Result<(), RpcError> {
    let mut tasks = vec![];
    for i in 0..limit {
        let transport_clone = transport.clone();
        let task = async move {
            let resp = transport_clone.call("ping", Value::Null)?;
            info!("Call {} got response: {:?}", i, resp.result);
            Ok::<(), RpcError>(())
        };
        tasks.push(task);
    }

    let resps: Vec<Result<(), RpcError>> =
        futures::future::join_all(tasks).await.into_iter().collect();

    for resp in resps {
        resp?;
    }

    Ok(())
}

async fn call_async(transport: Transport, _: ()) -> Result<Value, RpcError> {
    let resp = transport.call_async("ping", Value::Null).await?;
    Ok(resp.result)
}

async fn prime_sieve(_transport: Transport, limit: u64) -> Result<Value, RpcError> {
    let limit = limit as usize;
    let primes = sieve_of_eratosthenes(limit);
    info!("Generated {} primes up to {}", primes.len(), limit);
    Ok(serde_json::json!({
        "count": primes.len(),
        "limit": limit
    }))
}

fn sieve_of_eratosthenes(limit: usize) -> Vec<usize> {
    if limit < 2 {
        return vec![];
    }

    let mut is_prime = vec![true; limit + 1];
    is_prime[0] = false;
    is_prime[1] = false;

    for i in 2..=((limit as f64).sqrt() as usize) {
        if is_prime[i] {
            for j in ((i * i)..=limit).step_by(i) {
                is_prime[j] = false;
            }
        }
    }

    (2..=limit).filter(|&i| is_prime[i]).collect()
}

fn main() {
    fmt()
        .with_max_level(LevelFilter::TRACE)
        .with_writer(stderr)
        .compact()
        .with_ansi(false)
        .without_time()
        .init();
    info!("Starting plugin...");

    PluginServer::new()
        .with_method("ping", ping)
        .with_method("get_random_number", get_random_number)
        .with_method("get_time", get_time)
        .with_method("sleep", sleep)
        .with_method("call", call)
        .with_method("call_many", call_many)
        .with_method("call_async", call_async)
        .with_method("prime_sieve", prime_sieve)
        .run();
}
