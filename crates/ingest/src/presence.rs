/// Native field presence. Missing, explicit null and a supplied value can have
/// different replay semantics; optional values must not erase that distinction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence<T> {
    Missing,
    Null,
    Value(T),
}

impl<T> Presence<T> {
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Presence<U> {
        match self {
            Self::Missing => Presence::Missing,
            Self::Null => Presence::Null,
            Self::Value(value) => Presence::Value(f(value)),
        }
    }
}
