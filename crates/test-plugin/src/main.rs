use rand::Rng;
use serde_json::{self, Value};
use std::io::stderr;
use tokio::runtime::Builder;
use tracing::{info, level_filters::LevelFilter};
use tracing_subscriber::fmt;
use wasmi_plugin_pdk::{client::Client, rpc_message::RpcError, server::Server};

async fn ping(_: &mut Client, _: ()) -> Result<Value, RpcError> {
    Ok(Value::String("pong".to_string()))
}

async fn get_random_number(_: &mut Client, _: ()) -> Result<Value, RpcError> {
    let mut rng = rand::rng();
    let random_number: u64 = rng.random();
    Ok(Value::Number(random_number.into()))
}

async fn get_time(_: &mut Client, _: ()) -> Result<Value, RpcError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| RpcError::custom(format!("SystemTime before UNIX EPOCH!: {}", e)))?;
    Ok(Value::Number(now.as_secs().into()))
}

async fn sleep(_: &mut Client, duration_ms: u64) -> Result<(), RpcError> {
    info!("Sleeping for {} milliseconds...", duration_ms);
    std::thread::sleep(std::time::Duration::from_millis(duration_ms));
    info!("Woke up after sleeping for {} milliseconds", duration_ms);
    Ok(())
}

async fn tokio_sleep(_: &mut Client, duration_ms: u64) -> Result<(), RpcError> {
    info!("Sleeping for {} milliseconds...", duration_ms);
    tokio::time::sleep(std::time::Duration::from_millis(duration_ms)).await;
    info!("Woke up after sleeping for {} milliseconds", duration_ms);
    Ok(())
}

async fn many_echo(client: &mut Client, limit: u64) -> Result<(), RpcError> {
    for i in 0..limit {
        let resp = client.call("echo", Value::Number(i.into()))?;

        if resp.id != i {
            return Err(RpcError::custom(format!(
                "Incorrect response id: expected {}, got {}",
                i, resp.id
            )));
        }

        if resp.result != Value::Number(i.into()) {
            return Err(RpcError::custom(format!(
                "Incorrect response result: {:?}",
                resp.result
            )));
        }
    }

    Ok(())
}

async fn prime_sieve(_: &mut Client, limit: u64) -> Result<Value, RpcError> {
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

    Server::new()
        .with_method("ping", ping)
        .with_method("get_random_number", get_random_number)
        .with_method("get_time", get_time)
        .with_method("sleep", sleep)
        .with_method("tokio_sleep", tokio_sleep)
        .with_method("many_echo", many_echo)
        .with_method("prime_sieve", prime_sieve)
        .run();
}
