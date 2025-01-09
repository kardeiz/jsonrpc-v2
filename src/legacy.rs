pub use futures::future::TryFutureExt;
pub use futures::stream::TryStreamExt;
pub use bytes_v05::Bytes;
pub use factory::actix::*;


#[cfg(feature = "actix-web-v2-integration")]
    /// Converts the server into an `actix-web` compatible `NewService`
    pub fn into_actix_web_service(
        self: Arc<Self>,
    ) -> impl actix_service_v1::ServiceFactory<
        Request = actix_web_v2::dev::ServiceRequest,
        Response = actix_web_v2::dev::ServiceResponse,
        Error = actix_web_v2::Error,
        Config = (),
        InitError = (),
    > {
        let service = Arc::clone(&self);

        let inner = move |req: actix_web_v2::dev::ServiceRequest| {
            let service = Arc::clone(&service);
            let (req, payload) = req.into_parts();
            let rt = payload
                .map_err(actix_web_v2::Error::from)
                .try_fold(actix_web_v2::web::BytesMut::new(), move |mut body, chunk| async move {
                    body.extend_from_slice(&chunk);
                    Ok::<_, actix_web_v2::Error>(body)
                })
                .and_then(move |bytes| {
                    service.handle_bytes(bytes.freeze()).map(|res| match res {
                        ResponseObjects::Empty => Ok(actix_web_v2::dev::ServiceResponse::new(
                            req,
                            actix_web_v2::HttpResponse::NoContent().finish(),
                        )),
                        json => Ok(actix_web_v2::dev::ServiceResponse::new(
                            req,
                            actix_web_v2::HttpResponse::Ok().json(json),
                        )),
                    })
                });
            rt
        };

        actix_service_v1::fn_service::<_, _, _, _, _, _>(inner)
    }

    #[cfg(feature = "actix-web-v3-integration")]
    /// Converts the server into an `actix-web` compatible `NewService`
    pub fn into_actix_web_service(
        self: Arc<Self>,
    ) -> impl actix_service_v1::ServiceFactory<
        Request = actix_web_v3::dev::ServiceRequest,
        Response = actix_web_v3::dev::ServiceResponse,
        Error = actix_web_v3::Error,
        Config = (),
        InitError = (),
    > {
        let service = Arc::clone(&self);

        let inner = move |req: actix_web_v3::dev::ServiceRequest| {
            let service = Arc::clone(&service);
            let (req, payload) = req.into_parts();
            let rt = payload
                .map_err(actix_web_v3::Error::from)
                .try_fold(actix_web_v3::web::BytesMut::new(), move |mut body, chunk| async move {
                    body.extend_from_slice(&chunk);
                    Ok::<_, actix_web_v3::Error>(body)
                })
                .and_then(move |bytes| {
                    service.handle_bytes(bytes.freeze()).map(|res| match res {
                        ResponseObjects::Empty => Ok(actix_web_v3::dev::ServiceResponse::new(
                            req,
                            actix_web_v3::HttpResponse::NoContent().finish(),
                        )),
                        json => Ok(actix_web_v3::dev::ServiceResponse::new(
                            req,
                            actix_web_v3::HttpResponse::Ok().json(json),
                        )),
                    })
                });
            rt
        };

        actix_service_v1::fn_service::<_, _, _, _, _, _>(inner)
    }