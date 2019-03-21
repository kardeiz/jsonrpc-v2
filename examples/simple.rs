use actix::prelude::*;
use futures::future::Future;
use jsonrpc_v2::*;

use futures::future::IntoFuture;

fn add(Params(params): Params<(usize, usize)>, state: State<AppState>) -> Result<usize, Error> {
    Ok(params.0 + params.1 + state.num)
}

fn forty_two(_params: Params<()>, ctx: (State<AppState>, ())) -> Result<usize, Error> { Err(42.into()) }

pub struct AppState {
    num: usize
}


fn static_fn(req: actix_web::HttpRequest) -> actix_web::HttpResponse { 
    let json = serde_json::json!{{
        "jsonrpc": "2.0", "result": 26, "id": null
    }};
    actix_web::HttpResponse::Ok().json(json)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {

    let rpc = Server::with_state(AppState { num: 23 })
        .with_method("forty_two", forty_two)
        .with_method("add", add)
        .finish();

    actix_web::server::new(move || {
        let rpc = rpc.clone();
        let json_only = actix_web::pred::Header("Content-Type", "application/json");
        actix_web::App::new()
            .resource("/api", |r| r.post().filter(json_only).h(rpc))
            .route("/static", actix_web::http::Method::GET, static_fn)
    })
    .bind("0.0.0.0:3000")?
    .run();

    Ok(())
}
