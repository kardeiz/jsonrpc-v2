pub use futures::future::TryFutureExt;
pub use futures::stream::TryStreamExt;

pub use bytes_v10::Bytes;

use futures::future::FutureExt;
use futures::Future;
use serde::Serialize;
use std::sync::Arc;

use crate::{
    BoxedSerialize, Error, Factory, FromRequest, Handler, RequestObjectWithData, ResponseObjects,
    Router, Server,
};

/// Wrapper around `actix_web::HttpRequest`
#[derive(Clone)]
pub struct HttpRequest(pub actix_web_v4::HttpRequest);

impl std::ops::Deref for HttpRequest {
    type Target = actix_web_v4::HttpRequest;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct BoxedHandler(
    pub  Arc<
        dyn Fn(
                RequestObjectWithData,
            )
                -> std::pin::Pin<Box<dyn Future<Output = Result<BoxedSerialize, Error>>>>
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
            }) as std::pin::Pin<Box<dyn Future<Output = Result<BoxedSerialize, Error>>>>
        };

        BoxedHandler(Arc::new(inner))
    }
}

impl<R> Server<R>
where
    R: Router + 'static,
{
    /// Converts the server into an `actix-web` compatible `NewService`
    pub fn into_web_service(
        self: Arc<Self>,
    ) -> impl actix_service_v2::ServiceFactory<
        actix_web_v4::dev::ServiceRequest,
        Response = actix_web_v4::dev::ServiceResponse,
        Error = actix_web_v4::Error,
        Config = (),
        InitError = (),
    > {
        let service = Arc::clone(&self);

        let inner = move |req: actix_web_v4::dev::ServiceRequest| {
            let service = Arc::clone(&service);
            let (req, payload) = req.into_parts();
            let rt = payload
                .map_err(actix_web_v4::Error::from)
                .try_fold(actix_web_v4::web::BytesMut::new(), move |mut body, chunk| async move {
                    body.extend_from_slice(&chunk);
                    Ok::<_, actix_web_v4::Error>(body)
                })
                .and_then(move |bytes| {
                    service.handle_bytes(bytes.freeze(), Some(HttpRequest(req.clone()))).map(
                        |res| match res {
                            ResponseObjects::Empty => Ok(actix_web_v4::dev::ServiceResponse::new(
                                req,
                                actix_web_v4::HttpResponse::NoContent().finish(),
                            )),
                            json => Ok(actix_web_v4::dev::ServiceResponse::new(
                                req,
                                actix_web_v4::HttpResponse::Ok().json(json),
                            )),
                        },
                    )
                });
            rt
        };

        actix_service_v2::fn_service::<_, _, _, _, _, _>(inner)
    }
}
