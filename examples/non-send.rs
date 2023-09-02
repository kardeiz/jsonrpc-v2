use actix_web_v4::{guard, web, App, HttpServer};
use jsonrpc_v2::{Error, Params, Server};
use std::rc::Rc;

async fn add(x: &usize, y: &usize) -> usize {
    x + y
}

async fn non_send(Params(params): Params<usize>) -> Result<usize, Error> {
    // Using a Rc to make sure this async block returns a future that is ?Send
    let foo: Rc<usize> = Rc::new(1);
    Ok(add(&params, foo.as_ref()).await)
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let rpc = Server::new().with_method("non_send", non_send).finish();

    HttpServer::new(move || {
        let rpc = rpc.clone();
        App::new().service(web::service("/api").guard(guard::Post()).finish(rpc.into_web_service()))
    })
    .bind("0.0.0.0:3000")?
    .run()
    .await
}
