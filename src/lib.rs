use std::ops::{Deref, DerefMut};

use axum::{
    async_trait,
    body::Full,
    extract::{FromRequest, RequestParts},
    response::{IntoResponse, Response},
    BoxError,
};
use axum::{
    body::{self, Bytes},
    http::{header::HeaderValue, StatusCode},
};

use hyper::header;
use rejection::{HeadersAlreadyExtracted, MsgPackRejection};
use serde::{de::DeserializeOwned, Serialize};

use crate::rejection::{InvalidMsgPackBody, MissingMsgPackContentType};

mod error;
mod rejection;

/// MessagePack Extractor / Response.
///
/// When used as an extractor, it can deserialize request bodies into some type that
/// implements [`serde::Deserialize`]. If the request body cannot be parsed, or value of the
/// `Content-Type` header does not match any of the `application/msgpack`, `application/x-msgpack`
/// or `application/*+msgpack` it will reject the request and return a `400 Bad Request` response.
///
/// # Extractor example
///
/// ```rust,no_run
/// use axum::{
///     routing::post,
///     Router,
/// };
/// use axum_msgpack::MsgPack;
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct CreateUser {
///     email: String,
///     password: String,
/// }
///
/// async fn create_user(MsgPack(payload): MsgPack<CreateUser>) {
///     // payload is a `CreateUser`
/// }
///
/// let app = Router::new().route("/users", post(create_user));
/// # async {
/// # axum::Server::bind(&"".parse().unwrap()).serve(app.into_make_service()).await.unwrap();
/// # };
/// ```
///
/// When used as a response, it can serialize any type that implements [`serde::Serialize`] to
/// `MsgPack`, and will automatically set `Content-Type: application/msgpack` header.
///
/// # Response example
///
/// ```
/// use axum::{
///     extract::Path,
///     routing::get,
///     Router,
/// };
/// use axum_msgpack::MsgPack;
/// use serde::Serialize;
/// use uuid::Uuid;
///
/// #[derive(Serialize)]
/// struct User {
///     id: Uuid,
///     username: String,
/// }
///
/// async fn get_user(Path(user_id) : Path<Uuid>) -> MsgPack<User> {
///     let user = find_user(user_id).await;
///     MsgPack(user)
/// }
///
/// async fn find_user(user_id: Uuid) -> User {
///     // ...
///     # unimplemented!()
/// }
///
/// let app = Router::new().route("/users/:id", get(get_user));
/// # async {
/// # axum::Server::bind(&"".parse().unwrap()).serve(app.into_make_service()).await.unwrap();
/// # };
/// # mod uuid {
/// # use serde::{Serialize, Deserialize};
/// # #[derive(Serialize, Deserialize)]
/// # pub struct Uuid;
/// # }
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct MsgPack<T>(pub T);

#[async_trait]
impl<T, B> FromRequest<B> for MsgPack<T>
where
    T: DeserializeOwned,
    B: axum::body::HttpBody + Send,
    B::Data: Send,
    B::Error: Into<BoxError>,
{
    type Rejection = MsgPackRejection;

    async fn from_request(req: &mut RequestParts<B>) -> Result<Self, Self::Rejection> {
        if message_pack_content_type(req)? {
            let bytes = Bytes::from_request(req).await?;
            let value = rmp_serde::from_read_ref(&bytes).map_err(InvalidMsgPackBody::from_err)?;

            Ok(MsgPack(value))
        } else {
            Err(MissingMsgPackContentType.into())
        }
    }
}

impl<T> Deref for MsgPack<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for MsgPack<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> From<T> for MsgPack<T> {
    fn from(inner: T) -> Self {
        Self(inner)
    }
}

impl<T> IntoResponse for MsgPack<T>
where
    T: Serialize,
{
    fn into_response(self) -> Response {
        let bytes = match rmp_serde::encode::to_vec_named(&self.0) {
            Ok(res) => res,
            Err(err) => {
                return Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header(header::CONTENT_TYPE, "text/plain")
                    .body(body::boxed(Full::from(err.to_string())))
                    .unwrap();
            }
        };
        
        let mut res = bytes.into_response();

        res.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/msgpack"),
        );
        res
    }
}

fn message_pack_content_type<B>(req: &RequestParts<B>) -> Result<bool, HeadersAlreadyExtracted> {
    let content_type = if let Some(content_type) = req
        .headers()
        .ok_or_else(HeadersAlreadyExtracted::default)?
        .get(header::CONTENT_TYPE)
    {
        content_type
    } else {
        return Ok(false);
    };

    let content_type = if let Ok(content_type) = content_type.to_str() {
        content_type
    } else {
        return Ok(false);
    };

    let mime = if let Ok(mime) = content_type.parse::<mime::Mime>() {
        mime
    } else {
        return Ok(false);
    };

    let is_message_pack = mime.type_() == "application"
        && (["msgpack", "x-msgpack"]
            .iter()
            .any(|subtype| *subtype == mime.subtype())
            || mime.suffix().map_or(false, |suffix| suffix == "msgpack"));

    Ok(is_message_pack)
}

#[cfg(test)]
mod tests {
    use axum::response::IntoResponse;
    use serde::{Serialize, Deserialize};
    use hyper::body::to_bytes;
    use crate::MsgPack;

    #[derive(Debug, Serialize, Deserialize, Clone)]
    struct Input { foo: String }

    #[tokio::test]
    async fn serializes_named() {
        let input = Input { foo: "bar".into()};
        let serialized = rmp_serde::encode::to_vec_named(&input);
        assert!(serialized.is_ok());
        let serialized = serialized.unwrap();
        
        let body = MsgPack(input).into_response().into_body();
        let bytes = to_bytes(body).await;
        assert!(bytes.is_ok());
        let bytes = bytes.unwrap();

        assert_eq!(serialized, bytes);
    }
}
