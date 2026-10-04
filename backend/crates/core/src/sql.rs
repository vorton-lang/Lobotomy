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

#[cfg(test)]
mod tests {
    use crate::task::Phase;

    #[test]
    fn one_name_serves_the_database_serde_and_parse() {
        for phase in [Phase::Queued, Phase::Executing, Phase::Abandoned] {
            assert_eq!(Phase::parse(phase.as_str()).unwrap(), phase);
            assert_eq!(serde_json::to_value(phase).unwrap(), phase.as_str());
        }
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let read: Phase = conn.query_row("SELECT ?1", [Phase::Verifying], |r| r.get(0)).unwrap();
        assert_eq!(read, Phase::Verifying);
        assert!(matches!(Phase::parse("reviewing"), Err(crate::Error::Invariant(_))));
        let unknown: rusqlite::Result<Phase> = conn.query_row("SELECT 'reviewing'", [], |r| r.get(0));
        assert!(unknown.is_err(), "a name the code does not know fails the read");
    }
}
