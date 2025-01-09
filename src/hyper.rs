pub struct BoxedHandler(
    Box<
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

        BoxedHandler(Box::new(inner))
    }
}


/// Converts the server into an `actix-web` compatible `NewService`
pub fn into_hyper_web_service<R: Router + 'static>(server: Arc<Server<R>>) -> Hyper<R> {
    Hyper(self)
}

pub struct Hyper<R>(pub(crate) Arc<Server<R>>);

impl<R> hyper::service::Service<hyper::Request<hyper::Body>> for Hyper<R>
where
    R: Router + Send + Sync + 'static,
{
    type Response = hyper::Response<hyper::Body>;
    type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
    type Future =
        std::pin::Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: hyper::Request<hyper::Body>) -> Self::Future {
        use hyper::body::HttpBody;

        let service = Arc::clone(&self.0);

        let rt = async move {
            let mut buf = if let Some(content_length) = req
                .headers()
                .get(hyper::header::CONTENT_LENGTH)
                .and_then(|x| x.to_str().ok())
                .and_then(|x| x.parse().ok())
            {
                bytes_v10::BytesMut::with_capacity(content_length)
            } else {
                bytes_v10::BytesMut::default()
            };

            let mut body = req.into_body();

            while let Some(chunk) = body.data().await {
                buf.extend(chunk?);
            }

            match service.handle_bytes(buf.freeze()).await {
                ResponseObjects::Empty => hyper::Response::builder()
                    .status(hyper::StatusCode::NO_CONTENT)
                    .body(hyper::Body::from(Vec::<u8>::new()))
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
                json => serde_json::to_vec(&json)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                    .and_then(|json| {
                        hyper::Response::builder()
                            .status(hyper::StatusCode::OK)
                            .header("Content-Type", "application/json")
                            .body(hyper::Body::from(json))
                            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                    }),
            }
        };
        Box::pin(rt)
    }
}

impl<'a, R> tower_service::Service<&'a hyper::server::conn::AddrStream> for Hyper<R> {
    type Response = Hyper<R>;
    type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
    type Future = future::Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: &'a hyper::server::conn::AddrStream) -> Self::Future {
        future::ready(Ok(Hyper(Arc::clone(&self.0))))
    }
}
