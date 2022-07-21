use super::*;

pub use actix_web_v4::HttpRequest;

use type_map::concurrent::TypeMap;
use std::sync::Arc;

/// A trait to extract data from the request
#[async_trait::async_trait(?Send)]
pub trait FromRequest: Sized {
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error>;
}

#[async_trait::async_trait(?Send)]
impl FromRequest for () {
    async fn from_request(_: &RequestObjectWithData) -> Result<Self, Error> {
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl<T: Send + Sync + 'static> FromRequest for HttpRequestLocalData<T> {
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        let out = req
            .http_request_local_data
            .as_ref()
            .and_then(|x| x.get::<Arc<T>>())
            .map(|x| HttpRequestLocalData(Arc::clone(&x)))
            .ok_or_else(|| {
                Error::internal(format!("Missing data for: `{}`", std::any::type_name::<T>()))
            })?;
        Ok(out)
    }
}

#[async_trait::async_trait(?Send)]
impl<T: Send + Sync + 'static> FromRequest for Data<T> {
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        let out = req.data.as_ref().and_then(|x| x.get::<Data<T>>()).map(|x| Data(Arc::clone(&x.0))).ok_or_else(|| {
            Error::internal(format!("Missing data for: `{}`", std::any::type_name::<T>()))
        })?;
        Ok(out)
    }
}

// #[async_trait::async_trait]
// impl FromRequest for LocalData {
//     async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
//         let out = req
//             .local_data
//             .clone()
//             .ok_or_else(|| Error::internal(format!("Missing data for: `LocalData`")))?;

//         Ok(out)
//     }
// }

#[async_trait::async_trait(?Send)]
impl<T: DeserializeOwned> FromRequest for Params<T> {
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        Ok(Self::from_request_object(&req.inner)?)
    }
}

#[async_trait::async_trait(?Send)]
impl FromRequest for Id {
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        Ok(req
            .inner
            .id
            .clone()
            .and_then(|x| x)
            .ok_or_else(|| Error::internal("No `id` provided"))?)
    }
}

#[async_trait::async_trait(?Send)]
impl FromRequest for Method {
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        Ok(Method(req.inner.method.clone()))
    }
}

#[async_trait::async_trait(?Send)]
impl<T: FromRequest> FromRequest for Option<T> {
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        Ok(T::from_request(req).await.ok())
    }
}

#[async_trait::async_trait(?Send)]
impl<T1> FromRequest for (T1,)
where
    T1: FromRequest,
{
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        Ok((T1::from_request(req).await?,))
    }
}

#[async_trait::async_trait(?Send)]
impl<T1, T2> FromRequest for (T1, T2)
where
    T1: FromRequest,
    T2: FromRequest,
{
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        let (t1, t2) = futures::join!(T1::from_request(req), T2::from_request(req));
        Ok((t1?, t2?))
    }
}

#[async_trait::async_trait(?Send)]
impl<T1, T2, T3> FromRequest for (T1, T2, T3)
where
    T1: FromRequest,
    T2: FromRequest,
    T3: FromRequest,
{
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        let (t1, t2, t3) =
            futures::join!(T1::from_request(req), T2::from_request(req), T3::from_request(req));
        Ok((t1?, t2?, t3?))
    }
}

#[async_trait::async_trait(?Send)]
impl<T1, T2, T3, T4> FromRequest for (T1, T2, T3, T4)
where
    T1: FromRequest,
    T2: FromRequest,
    T3: FromRequest,
    T4: FromRequest,
{
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        let (t1, t2, t3, t4) = futures::join!(
            T1::from_request(req),
            T2::from_request(req),
            T3::from_request(req),
            T4::from_request(req)
        );
        Ok((t1?, t2?, t3?, t4?))
    }
}

#[async_trait::async_trait(?Send)]
impl<T1, T2, T3, T4, T5> FromRequest for (T1, T2, T3, T4, T5)
where
    T1: FromRequest,
    T2: FromRequest,
    T3: FromRequest,
    T4: FromRequest,
    T5: FromRequest,
{
    async fn from_request(req: &RequestObjectWithData) -> Result<Self, Error> {
        let (t1, t2, t3, t4, t5) = futures::join!(
            T1::from_request(req),
            T2::from_request(req),
            T3::from_request(req),
            T4::from_request(req),
            T5::from_request(req)
        );
        Ok((t1?, t2?, t3?, t4?, t5?))
    }
}

#[doc(hidden)]
#[async_trait::async_trait(?Send)]
pub trait Factory<S, E, T> {
    async fn call(&self, param: T) -> Result<S, E>;
}

#[doc(hidden)]
struct Handler<F, S, E, T>
where
    F: Factory<S, E, T>,
{
    hnd: F,
    _t: PhantomData<fn() -> (S, E, T)>,
}

impl<F, S, E, T> Handler<F, S, E, T>
where
    F: Factory<S, E, T>,
{
    fn new(hnd: F) -> Self {
        Handler { hnd, _t: PhantomData }
    }
}

#[async_trait::async_trait(?Send)]
impl<FN, I, S, E> Factory<S, E, ()> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>>,
    FN: Fn() -> I,
{
    async fn call(&self, _: ()) -> Result<S, E> {
        (self)().await
    }
}

#[async_trait::async_trait(?Send)]
impl<FN, I, S, E, T1> Factory<S, E, (T1,)> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>>,
    FN: Fn(T1) -> I,
    T1: 'static,
{
    async fn call(&self, param: (T1,)) -> Result<S, E> {
        (self)(param.0).await
    }
}

#[async_trait::async_trait(?Send)]
impl<FN, I, S, E, T1, T2> Factory<S, E, (T1, T2)> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>>,
    FN: Fn(T1, T2) -> I,
    T1: 'static,
    T2: 'static,
{
    async fn call(&self, param: (T1, T2)) -> Result<S, E> {
        (self)(param.0, param.1).await
    }
}

#[async_trait::async_trait(?Send)]
impl<FN, I, S, E, T1, T2, T3> Factory<S, E, (T1, T2, T3)> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>>,
    FN: Fn(T1, T2, T3) -> I,
    T1: 'static,
    T2: 'static,
    T3: 'static,
{
    async fn call(&self, param: (T1, T2, T3)) -> Result<S, E> {
        (self)(param.0, param.1, param.2).await
    }
}

#[async_trait::async_trait(?Send)]
impl<FN, I, S, E, T1, T2, T3, T4> Factory<S, E, (T1, T2, T3, T4)> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>>,
    FN: Fn(T1, T2, T3, T4) -> I,
    T1: 'static,
    T2: 'static,
    T3: 'static,
    T4: 'static,
{
    async fn call(&self, param: (T1, T2, T3, T4)) -> Result<S, E> {
        (self)(param.0, param.1, param.2, param.3).await
    }
}

#[async_trait::async_trait(?Send)]
impl<FN, I, S, E, T1, T2, T3, T4, T5> Factory<S, E, (T1, T2, T3, T4, T5)> for FN
where
    S: 'static,
    E: 'static,
    I: Future<Output = Result<S, E>>,
    FN: Fn(T1, T2, T3, T4, T5) -> I,
    T1: 'static,
    T2: 'static,
    T3: 'static,
    T4: 'static,
    T5: 'static,
{
    async fn call(&self, param: (T1, T2, T3, T4, T5)) -> Result<S, E> {
        (self)(param.0, param.1, param.2, param.3, param.4).await
    }
}

impl<F, S, E, T> From<Handler<F, S, E, T>> for BoxedHandler
where
    F: Factory<S, E, T> + Send + Sync + 'static,
    S: Serialize + Send + 'static,
    Error: From<E>,
    E: 'static,
    T: FromRequest + 'static,
{
    fn from(t: Handler<F, S, E, T>) -> BoxedHandler {
        let hnd = Arc::new(t.hnd);

        let inner = move |req: RequestObjectWithData| {
            let hnd = Arc::clone(&hnd);
            async move {
                let out = {
                    let param = T::from_request(&req).await?;
                    hnd.call(param).await?
                };
                Ok(Box::new(out) as BoxedSerialize)
            }.boxed_local()
        };

        BoxedHandler(Arc::new(inner))
    }
}

pub struct BoxedHandler(
    Arc<
        dyn Fn(
                RequestObjectWithData,
            )
                -> std::pin::Pin<Box<dyn Future<Output = Result<BoxedSerialize, Error>>>>
        + Send + Sync
    >,
);

pub struct MapRouter(HashMap<String, BoxedHandler>);

impl Default for MapRouter {
    fn default() -> Self {
        MapRouter(HashMap::default())
    }
}

pub trait Router: Default {
    fn get(&self, name: &str) -> Option<&BoxedHandler>;
    fn insert(&mut self, name: String, handler: BoxedHandler) -> Option<BoxedHandler>;
}

impl Router for MapRouter {
    fn get(&self, name: &str) -> Option<&BoxedHandler> {
        self.0.get(name)
    }
    fn insert(&mut self, name: String, handler: BoxedHandler) -> Option<BoxedHandler> {
        self.0.insert(name, handler)
    }
}

/// Server/request handler
pub struct Server<R> {
    data: Option<Arc<TypeMap>>,
    router: R,
    extract_from_http_request_fns: Option<
        Vec<
            Box<
                dyn Fn(
                        &'_ HttpRequest,
                    ) -> std::pin::Pin<
                        Box<
                            dyn Future<Output = Result<type_map::concurrent::KvPair, Error>>
                        >
                    > + Send + Sync
            >,
        >,
    >,
}

/// Builder used to add methods to a server
///
/// Created with `Server::new` or `Server::with_state`
pub struct ServerBuilder<R> {
    data: Option<TypeMap>,
    router: R,
    extract_from_http_request_fns: Option<
        Vec<
            Box<
                dyn Fn(
                        &'_ HttpRequest,
                    ) -> std::pin::Pin<
                        Box<
                            dyn Future<Output = Result<type_map::concurrent::KvPair, Error>>,
                        >,
                    > + Send + Sync
            >,
        >,
    >,
}

impl Server<MapRouter> {
    pub fn new() -> ServerBuilder<MapRouter> {
        Server::with_router(MapRouter::default())
    }
}

impl<R: Router> Server<R> {
    pub fn with_router(router: R) -> ServerBuilder<R> {
        ServerBuilder { data: None, router, extract_from_http_request_fns: None }
    }
}

impl<R: Router> ServerBuilder<R> {
    /// Add a data/state storage container to the server
    pub fn with_data<T: Send + Sync + 'static>(mut self, data: Data<T>) -> Self {
        let mut map = self.data.take().unwrap_or_else(type_map::concurrent::TypeMap::new);
        map.insert(data);
        self.data = Some(map);
        self
    }

    pub fn with_extract_from_http_request_fn<T, U, V>(mut self, fn_: T) -> Self
    where
        U: Send + Sync + 'static,
        V: Future<Output = Result<U, Error>> + 'static,
        T: Fn(&'_ HttpRequest) -> V + Send + Sync + 'static,
    {
        let mut extract_from_http_request_fns =
            self.extract_from_http_request_fns.take().unwrap_or_else(Vec::new);

        extract_from_http_request_fns.push(Box::new(move |req| {
            fn_(req).map_ok(|x| type_map::concurrent::KvPair::new(Arc::new(x))).boxed_local()
        }));

        self.extract_from_http_request_fns = Some(extract_from_http_request_fns);
        self
    }

    /// Add a method handler to the server
    ///
    /// The method is an async function that takes up to 5 [`FromRequest`](trait.FromRequest.html) items
    /// and returns a value that can be resolved to a `TryFuture`, where `TryFuture::Ok` is a serializable object, e.g.:
    ///
    /// ```rust,no_run
    /// async fn handle(params: Params<(i32, String)>, data: Data<HashMap<String, String>>) -> Result<String, Error> { /* ... */ }
    /// ```
    pub fn with_method<N, S, E, F, T>(mut self, name: N, handler: F) -> Self
    where
        N: Into<String>,
        F: Factory<S, E, T> + Send + Sync + 'static,
        S: Serialize + Send + 'static,
        Error: From<E>,
        E: 'static,
        T: FromRequest + 'static,
    {
        self.router.insert(name.into(), Handler::new(handler).into());
        self
    }

    /// Convert the server builder into the finished struct, wrapped in an `Arc`
    pub fn finish(self) -> Arc<Server<R>> {
        let ServerBuilder { router, data, extract_from_http_request_fns } = self;
        Arc::new(Server { router, data: data.map(Arc::new), extract_from_http_request_fns })
    }

    /// Convert the server builder into the finished struct
    pub fn finish_unwrapped(self) -> Server<R> {
        let ServerBuilder { router, data, extract_from_http_request_fns } = self;
        Server { router, data: data.map(Arc::new), extract_from_http_request_fns }
    }
}


impl<R> Server<R>
where
    R: Router + 'static,
{
    #[cfg(feature = "actix-web-v1-integration")]
    fn handle_bytes_compat(
        &self,
        bytes: Bytes,
    ) -> impl futures_v01::Future<Item = ResponseObjects, Error = ()> {
        self.handle_bytes(bytes).unit_error().boxed().compat()
    }

    /// Handle requests, and return appropriate responses
    pub fn handle<I: Into<RequestKind>>(&self, req: I) -> impl Future<Output = ResponseObjects> {
        match req.into() {
            RequestKind::Bytes(bytes) => future::Either::Left(self.handle_bytes(bytes, None)),
            RequestKind::RequestObject(req) => future::Either::Right(future::Either::Left(
                self.handle_request_object(req, None).map(From::from),
            )),
            RequestKind::ManyRequestObjects(reqs) => future::Either::Right(future::Either::Right(
                self.handle_many_request_objects(reqs, None).map(From::from),
            )),
        }
    }

    fn handle_request_object(
        &self,
        req: RequestObject,
        http_req_opt: Option<HttpRequestWrapper>,
    ) -> impl Future<Output = SingleResponseObject> {
        let http_request_local_data_fut =
            match (self.extract_from_http_request_fns.as_ref(), http_req_opt) {
                (Some(extract_from_http_request_fns), Some(http_req)) => future::Either::Left(
                    futures::future::try_join_all(
                        extract_from_http_request_fns.iter().map(move |fn_| fn_(&http_req.0)),
                    )
                    .map_ok(|vs| {
                        let mut map = TypeMap::new();
                        for v in vs {
                            map.insert_kv_pair(v);
                        }
                        Some(Arc::new(map))
                    })
                    .unwrap_or_else(|_| None),
                ),
                _ => future::Either::Right(future::ready::<Option<Arc<TypeMap>>>(None)),
            };

        let data = self.data.clone();

        let opt_id = match req.id {
            Some(Some(ref id)) => Some(id.clone()),
            Some(None) => Some(Id::Null),
            None => None,
        };

        if let Some(handler) = self.router.get(req.method.as_ref()) {
            let handler = Arc::clone(&handler.0);

            let out = http_request_local_data_fut
                .map(|http_request_local_data| RequestObjectWithData {
                    inner: req,
                    data,
                    http_request_local_data
                })
                .then(move |req| handler(req))
                .then(|res| match res {
                    Ok(val) => future::ready(SingleResponseObject::result(val, opt_id)),
                    Err(e) => future::ready(SingleResponseObject::error(e, opt_id)),
                });
            future::Either::Left(out)
        } else {
            future::Either::Right(future::ready(SingleResponseObject::error(
                Error::METHOD_NOT_FOUND,
                opt_id,
            )))
        }
    }

    fn handle_many_request_objects<I: IntoIterator<Item = RequestObject>>(
        &self,
        reqs: I,
        http_req_opt: Option<HttpRequestWrapper>,
    ) -> impl Future<Output = ManyResponseObjects> {
        reqs.into_iter()
            .map(|r| self.handle_request_object(r, http_req_opt.clone()))
            .collect::<futures::stream::FuturesUnordered<_>>()
            .filter_map(|res| async move {
                match res {
                    SingleResponseObject::One(r) => Some(r),
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
            .map(|vec| {
                if vec.is_empty() {
                    ManyResponseObjects::Empty
                } else {
                    ManyResponseObjects::Many(vec)
                }
            })
    }

    fn handle_bytes(
        &self,
        bytes: Bytes,
        http_req_opt: Option<HttpRequestWrapper>,
    ) -> impl Future<Output = ResponseObjects> {
        if let Ok(raw_values) = OneOrManyRawValues::try_from_slice(bytes.as_ref()) {
            match raw_values {
                OneOrManyRawValues::Many(raw_reqs) => {
                    if raw_reqs.is_empty() {
                        return future::Either::Left(future::ready(ResponseObjects::One(
                            ResponseObject::error(Error::INVALID_REQUEST, Id::Null),
                        )));
                    }

                    let (okays, errs) = raw_reqs
                        .into_iter()
                        .map(|x| {
                            serde_json::from_str::<BytesRequestObject>(x.get())
                                .map(RequestObject::from)
                        })
                        .partition::<Vec<_>, _>(|x| x.is_ok());

                    let errs = errs
                        .into_iter()
                        .map(|_| ResponseObject::error(Error::INVALID_REQUEST, Id::Null))
                        .collect::<Vec<_>>();

                    future::Either::Right(future::Either::Left(
                        self.handle_many_request_objects(
                            okays.into_iter().flat_map(|x| x),
                            http_req_opt,
                        )
                        .map(|res| match res {
                            ManyResponseObjects::Many(mut many) => {
                                many.extend(errs);
                                ResponseObjects::Many(many)
                            }
                            ManyResponseObjects::Empty => {
                                if errs.is_empty() {
                                    ResponseObjects::Empty
                                } else {
                                    ResponseObjects::Many(errs)
                                }
                            }
                        }),
                    ))
                }
                OneOrManyRawValues::One(raw_req) => {
                    match serde_json::from_str::<BytesRequestObject>(raw_req.get())
                        .map(RequestObject::from)
                    {
                        Ok(rn) => future::Either::Right(future::Either::Right(
                            self.handle_request_object(rn, http_req_opt).map(|res| match res {
                                SingleResponseObject::One(r) => ResponseObjects::One(r),
                                _ => ResponseObjects::Empty,
                            }),
                        )),
                        Err(_) => future::Either::Left(future::ready(ResponseObjects::One(
                            ResponseObject::error(Error::INVALID_REQUEST, Id::Null),
                        ))),
                    }
                }
            }
        } else {
            future::Either::Left(future::ready(ResponseObjects::One(ResponseObject::error(
                Error::PARSE_ERROR,
                Id::Null,
            ))))
        }
    }

    #[cfg(feature = "actix-web-v1-integration")]
    /// Converts the server into an `actix-web` compatible `NewService`
    pub fn into_actix_web_service(
        self: Arc<Self>,
    ) -> impl actix_service_v04::NewService<
        Request = actix_web_v1::dev::ServiceRequest,
        Response = actix_web_v1::dev::ServiceResponse,
        Error = actix_web_v1::Error,
        Config = (),
        InitError = (),
    > {
        use futures_v01::{Future, Stream};

        let service = Arc::clone(&self);

        let inner = move |req: actix_web_v1::dev::ServiceRequest| {
            let service = Arc::clone(&service);
            let (req, payload) = req.into_parts();
            let rt = payload
                .map_err(actix_web_v1::Error::from)
                .fold(actix_web_v1::web::BytesMut::new(), move |mut body, chunk| {
                    body.extend_from_slice(&chunk);
                    Ok::<_, actix_web_v1::Error>(body)
                })
                .and_then(move |bytes| {
                    service.handle_bytes_compat(bytes.freeze()).then(|res| match res {
                        Ok(res_inner) => match res_inner {
                            ResponseObjects::Empty => Ok(actix_web_v1::dev::ServiceResponse::new(
                                req,
                                actix_web_v1::HttpResponse::NoContent().finish(),
                            )),
                            json => Ok(actix_web_v1::dev::ServiceResponse::new(
                                req,
                                actix_web_v1::HttpResponse::Ok().json(json),
                            )),
                        },
                        Err(_) => Ok(actix_web_v1::dev::ServiceResponse::new(
                            req,
                            actix_web_v1::HttpResponse::InternalServerError().into(),
                        )),
                    })
                });
            rt
        };

        actix_service_v04::service_fn::<_, _, _, ()>(inner)
    }

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

    #[cfg(feature = "actix-web-v4-integration")]
    /// Converts the server into an `actix-web` compatible `NewService`
    pub fn into_actix_web_service(
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
                    service.handle_bytes(bytes.freeze(), Some(HttpRequestWrapper(req.clone()))).map(
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

    #[cfg(all(
        feature = "actix-web-v1-integration",
        not(feature = "hyper-integration"),
        not(feature = "actix-web-v2-integration"),
        not(feature = "actix-web-v3-integration"),
        not(feature = "actix-web-v4-integration")
    ))]
    /// Is an alias to `into_actix_web_service` or `into_hyper_web_service` depending on which feature is enabled
    ///
    /// Is not provided when both features are enabled
    pub fn into_web_service(
        self: Arc<Self>,
    ) -> impl actix_service_v04::NewService<
        Request = actix_web_v1::dev::ServiceRequest,
        Response = actix_web_v1::dev::ServiceResponse,
        Error = actix_web_v1::Error,
        Config = (),
        InitError = (),
    > {
        self.into_actix_web_service()
    }

    #[cfg(all(
        feature = "actix-web-v2-integration",
        not(feature = "hyper-integration"),
        not(feature = "actix-web-v1-integration"),
        not(feature = "actix-web-v3-integration"),
        not(feature = "actix-web-v4-integration")
    ))]
    /// Is an alias to `into_actix_web_service` or `into_hyper_web_service` depending on which feature is enabled
    ///
    /// Is not provided when both features are enabled
    pub fn into_web_service(
        self: Arc<Self>,
    ) -> impl actix_service_v1::ServiceFactory<
        Request = actix_web_v2::dev::ServiceRequest,
        Response = actix_web_v2::dev::ServiceResponse,
        Error = actix_web_v2::Error,
        Config = (),
        InitError = (),
    > {
        self.into_actix_web_service()
    }

    #[cfg(all(
        feature = "actix-web-v3-integration",
        not(feature = "hyper-integration"),
        not(feature = "actix-web-v1-integration"),
        not(feature = "actix-web-v2-integration"),
        not(feature = "actix-web-v4-integration")
    ))]
    /// Is an alias to `into_actix_web_service` or `into_hyper_web_service` depending on which feature is enabled
    ///
    /// Is not provided when both features are enabled
    pub fn into_web_service(
        self: Arc<Self>,
    ) -> impl actix_service_v1::ServiceFactory<
        Request = actix_web_v3::dev::ServiceRequest,
        Response = actix_web_v3::dev::ServiceResponse,
        Error = actix_web_v3::Error,
        Config = (),
        InitError = (),
    > {
        self.into_actix_web_service()
    }

    #[cfg(all(
        feature = "actix-web-v4-integration",
        not(feature = "hyper-integration"),
        not(feature = "actix-web-v1-integration"),
        not(feature = "actix-web-v2-integration"),
        not(feature = "actix-web-v3-integration")
    ))]
    /// Is an alias to `into_actix_web_service` or `into_hyper_web_service` depending on which feature is enabled
    ///
    /// Is not provided when both features are enabled
    pub fn into_web_service(
        self: Arc<Self>,
    ) -> impl actix_service_v2::ServiceFactory<
        actix_web_v4::dev::ServiceRequest,
        Response = actix_web_v4::dev::ServiceResponse,
        Error = actix_web_v4::Error,
        Config = (),
        InitError = (),
    > {
        self.into_actix_web_service()
    }

}