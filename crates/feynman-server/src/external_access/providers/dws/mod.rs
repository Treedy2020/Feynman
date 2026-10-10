pub mod access;
pub mod authorization;
pub mod messages;
pub mod search;

pub fn search_args(value: serde_json::Value) -> feynman_core::PlatformResult<Vec<String>> {
    search::validated_args(value)
}
