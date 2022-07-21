use actix_web_v4::{guard, web, App, HttpServer, FromRequest};
use actix_identity::Identity;
use jsonrpc_v2::{Data, Error, HttpRequestLocalData, Params, Server};
use futures::TryFutureExt;

#[derive(serde::Deserialize)]
struct TwoNums {
    a: usize,
    b: usize,
}

pub struct Foo(String);

async fn test(Params(params): Params<serde_json::Value>) -> Result<String, Error> {
    Ok(serde_json::to_string_pretty(&params).unwrap())
}

async fn add(
    Params(params): Params<TwoNums>,
    req_path: HttpRequestLocalData<String>,
) -> Result<usize, Error> {
    dbg!(req_path.0);
    Ok(params.a + params.b)
}

async fn sub(Params(params): Params<(usize, usize)>) -> Result<usize, Error> {
    Ok(params.0 - params.1)
}

async fn message(data: Data<Foo>) -> Result<String, Error> {
    Ok(String::from(&(data.0).0))
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let rpc = Server::new()
        .with_data(Data::new(Foo(String::from("Hello!"))))
        .with_method("add", add)
        .with_method("sub", sub)
        .with_method("test", test)
        .with_method("message", message)
        .with_extract_from_http_request_fn(|req| 
            // futures::future::ok::<_, Error>(String::from(req.path()))
            <Identity as FromRequest>::extract(req)
                .map_err(|_| ())
                .and_then(|x| futures::future::ready(x.id()).map_err(|_| ()))
                .map_err(|_| Error::internal("No ID"))
        )
        .finish();

    HttpServer::new(move || {
        let rpc = rpc.clone();
        App::new()
            .wrap(actix_identity::IdentityMiddleware::default())
            .service(web::service("/api").guard(guard::Post()).finish(rpc.into_web_service()))
    })
    .bind("0.0.0.0:3000")?
    .run()
    .await
}