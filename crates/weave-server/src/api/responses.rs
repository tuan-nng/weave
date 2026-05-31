use serde::Serialize;

/// Consistent `{"data": ...}` envelope for all success responses.
#[derive(Serialize)]
pub struct DataResponse<T: Serialize> {
    pub data: T,
}
