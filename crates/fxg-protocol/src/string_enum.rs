//! Open string enums for values that can gain new variants over time.
//!
//! Unknown values are preserved as [`Unknown(String)`] instead of being
//! rejected or coerced to a default. The wire representation is the same as the
//! plain string field it replaces, so this is not a protocol change.

/// Defines a string enum with an `Unknown(String)` fallback.
///
/// The macro generates `as_str`, `Display`, `FromStr`, `Serialize`,
/// `Deserialize` and a `specta::Type` impl that emits `string` to TypeScript
/// (the UI can narrow the value itself when needed).
#[macro_export]
macro_rules! open_string_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident => $value:literal,
            )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        $vis enum $name {
            $(
                $(#[$variant_meta])*
                $variant,
            )*
            /// A value added by a newer peer / agent, preserved as-is.
            Unknown(::std::string::String),
        }

        impl $name {
            /// String representation used on the wire and in the database.
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $value,)*
                    Self::Unknown(value) => value.as_str(),
                }
            }

            /// Whether this value is one of the known variants.
            pub fn is_known(&self) -> bool {
                !matches!(self, Self::Unknown(_))
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = ::std::convert::Infallible;

            fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
                ::std::result::Result::Ok(match value {
                    $($value => Self::$variant,)*
                    other => Self::Unknown(other.to_owned()),
                })
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> ::std::result::Result<S::Ok, S::Error>
            where
                S: ::serde::Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
            where
                D: ::serde::Deserializer<'de>,
            {
                let value =
                    <::std::string::String as ::serde::Deserialize>::deserialize(deserializer)?;
                ::std::result::Result::Ok(
                    <Self as ::std::str::FromStr>::from_str(&value)
                        .unwrap_or_else(|never| match never {}),
                )
            }
        }

        impl ::specta::Type for $name {
            fn definition(
                types: &mut ::specta::Types,
            ) -> ::specta::datatype::DataType {
                <::std::string::String as ::specta::Type>::definition(types)
            }
        }
    };
}
