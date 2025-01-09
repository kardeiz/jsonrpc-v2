pub use futures::future::TryFutureExt;
pub use futures::stream::TryStreamExt;

pub use bytes_v10::Bytes;

use futures::FutureExt;
use futures::future;
use std::sync::Arc;
use futures::Future;
use serde::Serialize;
use http_body_util::{BodyExt, Full};

use crate::{Factory, Handler, RequestObjectWithData, Router, Error, BoxedSerialize, FromRequest, Server, ResponseObjects};

#[derive(Clone)]
pub struct HttpRequest(pub hyper::http::request::Parts);

impl std::ops::Deref for HttpRequest {
    type Target = hyper::http::request::Parts;
    
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct BoxedHandler(
    pub Arc<
        dyn Fn(
                RequestObjectWithData,
            )
                -> std::pin::Pin<Box<dyn Future<Output = Result<BoxedSerialize, Error>> + Send>>
            + Send
            + Sync,
    >,
);

impl<F, S, E, T> From<Handler<F, S, E, T>> for BoxedHandler
where
    F: Factory<S, E, T> + 'static + Send + Sync,
    S: Serialize + Send + 'static,
    Error: From<E>,
    E: 'static,
    T: FromRequest + 'static + Send,
{
    fn from(t: Handler<F, S, E, T>) -> BoxedHandler {
        let hnd = Arc::new(t.hnd);

        let inner = move |req: RequestObjectWithData| {
            let hnd = Arc::clone(&hnd);
            Box::pin(async move {
                let out = {
                    let param = T::from_request(&req).await?;
                    hnd.call(param).await?
                };
                Ok(Box::new(out) as BoxedSerialize)
            })
                as std::pin::Pin<Box<dyn Future<Output = Result<BoxedSerialize, Error>> + Send>>
        };

        BoxedHandler(Arc::new(inner))
    }
}

impl<R> Server<R>
where
    R: Router + 'static,
{
    /// Converts the server into an `actix-web` compatible `NewService`
    pub fn into_web_service(self: Arc<Self>) -> Hyper<R> {
        Hyper(self)
    }
}


pub struct Hyper<R>(pub(crate) Arc<Server<R>>);

impl<R> hyper::service::Service<hyper::Request<hyper::body::Incoming>> for Hyper<R>
where
    R: Router + Send + Sync + 'static,
{
    type Response = hyper::Response<Full<Bytes>>;
    type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
    type Future =
        std::pin::Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: hyper::Request<hyper::body::Incoming>) -> Self::Future {       

        let service = Arc::clone(&self.0);

        let (parts, body) = req.into_parts();

        let rt = body.collect().map_ok(|x| x.to_bytes())
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)    
            .and_then(move |bytes| {
            service.handle_bytes(bytes, Some(HttpRequest(parts.clone()))).map(|res| match res {
                ResponseObjects::Empty => hyper::Response::builder()
                    .status(hyper::StatusCode::NO_CONTENT)
                    .body(Full::new(Bytes::from(Vec::<u8>::new())))
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
                json => serde_json::to_vec(&json)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                    .and_then(|json| {
                        hyper::Response::builder()
                            .status(hyper::StatusCode::OK)
                            .header("Content-Type", "application/json")
                            .body(Full::new(Bytes::from(json)))
                            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                    }),
            })
        });
        Box::pin(rt)
    }
}

// impl<R> tower_service::Service<&'a hyper::server::conn::AddrStream> for Hyper<R> {
//     type Response = Hyper<R>;
//     type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
//     type Future = future::Ready<Result<Self::Response, Self::Error>>;

//     fn poll_ready(
//         &mut self,
//         _cx: &mut std::task::Context<'_>,
//     ) -> std::task::Poll<Result<(), Self::Error>> {
//         std::task::Poll::Ready(Ok(()))
//     }

//     fn call(&mut self, _: &'a hyper::server::conn::AddrStream) -> Self::Future {
//         future::ready(Ok(Hyper(Arc::clone(&self.0))))
//     }
// }
