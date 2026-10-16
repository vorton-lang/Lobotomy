//! Enums stored as text. One table of names per enum gives `as_str`, `parse`, the serde names and
//! `ToSql`/`FromSql`, so rows and parameters use the enum itself and the names cannot drift
//! between the four places that used to spell them out (#16).

/// Declares an enum whose values are stored as the given names. A stored name the code does not
/// know is a broken invariant: `parse` says so, and reading such a row fails.
macro_rules! sql_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $text:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        #[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
        $vis enum $name {
            $( $(#[$vmeta])* #[serde(rename = $text)] $variant ),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $( $name::$variant => $text ),+
                }
            }

            pub fn parse(s: &str) -> $crate::Result<Self> {
                match s {
                    $( $text => Ok($name::$variant), )+
                    other => Err($crate::Error::invariant(format!(concat!("unknown ", stringify!($name), " {:?}"), other))),
                }
            }
        }

        impl rusqlite::types::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                Ok(self.as_str().into())
            }
        }

        impl rusqlite::types::FromSql for $name {
            fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
                Self::parse(value.as_str()?).map_err(|e| rusqlite::types::FromSqlError::Other(Box::new(e)))
            }
        }
    };
}

pub(crate) use sql_enum;
