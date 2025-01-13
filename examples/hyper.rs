use hyper_util::rt::TokioIo;
use jsonrpc_v2::*;
use std::net::SocketAddr;
use tokio::net::TcpListener;

#[derive(serde::Deserialize)]
struct TwoNums {
    a: usize,
    b: usize,
}

async fn add(
    Params(params): Params<TwoNums>,
    req_path: HttpRequestLocalData<String>,
) -> Result<usize, Error> {
    dbg!(req_path.0);
    Ok(params.a + params.b)
}

async fn sub(
    Params(params): Params<(usize, usize)>,
    id: Option<jsonrpc_v2::Id>,
    method: jsonrpc_v2::Method,
) -> Result<usize, Error> {
    dbg!(id);
    dbg!(method.as_str());
    Ok(params.0 - params.1)
}

async fn message(data: Data<String>) -> Result<String, Error> {
    Ok(String::from(&*data))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc = Server::new()
        .with_data(Data::new(String::from("Hello!")))
        .with_method("add", add)
        .with_method("sub", sub)
        .with_method("message", message)
        .with_extract_from_http_request_fn(|req| {
            futures::future::ok::<_, Error>(String::from(req.uri.path()))
        })
        .finish();

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    // We create a TcpListener and bind it to 127.0.0.1:3000
    let listener = TcpListener::bind(addr).await?;

    // We start a loop to continuously accept incoming connections
    loop {
        let rpc = rpc.clone();
        let (stream, _) = listener.accept().await?;

        // Use an adapter to access something implementing `tokio::io` traits as if they implement
        // `hyper::rt` IO traits.
        let io = TokioIo::new(stream);

        // Spawn a tokio task to serve multiple connections concurrently
        tokio::task::spawn(async move {
            // Finally, we bind the incoming connection to our `hello` service
            if let Err(err) = ::hyper::server::conn::http1::Builder::new()
                // `service_fn` converts our function in a `Service`
                .serve_connection(io, rpc.into_web_service())
                .await
            {
                eprintln!("Error serving connection: {:?}", err);
            }
        });
    }

    // let addr = "0.0.0.0:3000".parse().unwrap();

    // let server = hyper::Server::bind(&addr).serve(rpc.into_web_service());

    // server.await?;

    Ok(())
}
