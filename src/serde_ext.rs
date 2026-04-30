use serde::Serialize;
use serde_json::Value;

pub trait Empty {
    fn is_empty(&self) -> bool;
}

impl Empty for String {
    fn is_empty(&self) -> bool {
        String::is_empty(self)
    }
}

impl<T> Empty for Vec<T> {
    fn is_empty(&self) -> bool {
        Vec::is_empty(self)
    }
}

pub fn is_empty<T>(value: &T) -> bool
where
    T: Empty + ?Sized,
{
    value.is_empty()
}

pub fn is_json_null(value: &Value) -> bool {
    value.is_null()
}

pub fn to_pretty_json<T: Serialize>(value: &T) -> serde_json::Result<String> {
    serde_json::to_string_pretty(value)
}
