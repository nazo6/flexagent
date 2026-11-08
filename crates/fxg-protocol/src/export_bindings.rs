//! Generates the UI TypeScript definitions
//! (`ui/src/lib/generated/bindings.ts`) from the `fxg-protocol` types.
//!
//! - The target types are those carrying `#[specta(collect)]` (plus any type
//!   they reference).
//! - BigInt-like integers such as `i64` / `u64` are numbers in JSON and the UI
//!   never goes through a `bigint` conversion, so they are emitted as `number`
//!   (specta errors on them by default to avoid precision loss, hence the
//!   explicit replacement with `Number`).
//! - `serde_json::Value` is emitted as `JsonValue` to match UI conventions.
//!
//! Run with: `cargo test -p fxg-protocol export_bindings` (or `mise run types`)

use std::path::PathBuf;

use specta::Type;
use specta::Types;
use specta::datatype::{
    DataType, Fields, Map, NamedDataType, NamedReferenceType, Primitive, Reference,
};
use specta_typescript::{Layout, Typescript};

/// Integer types that are emitted as the TS `number`.
///
/// By default specta treats BigInt-like integers as `bigint` to avoid precision
/// loss and errors out, and emits floats as `number | null` because JSON cannot
/// represent `NaN` / `Infinity`. In this protocol both are plain numbers in JSON
/// and the UI never converts them, so everything is emitted as `number`.
fn is_ts_number(p: &Primitive) -> bool {
    matches!(
        p,
        Primitive::i64
            | Primitive::u64
            | Primitive::i128
            | Primitive::u128
            | Primitive::isize
            | Primitive::usize
            | Primitive::f32
            | Primitive::f64
    )
}

/// The `DataType` representing `specta_typescript::Number` (the TS `number`).
fn ts_number() -> DataType {
    <specta_typescript::Number as Type>::definition(&mut Types::default())
}

fn remap_fields(fields: Fields, number: &DataType) -> Fields {
    fn fix(field: &mut specta::datatype::Field, number: &DataType) {
        if let Some(ty) = field.ty.take() {
            field.ty = Some(remap(ty, number));
        }
    }

    match fields {
        Fields::Unit => Fields::Unit,
        Fields::Unnamed(mut f) => {
            for field in &mut f.fields {
                fix(field, number);
            }
            Fields::Unnamed(f)
        }
        Fields::Named(mut f) => {
            for (_, field) in &mut f.fields {
                fix(field, number);
            }
            Fields::Named(f)
        }
    }
}

/// Recursively replaces BigInt-like integers with `number`.
fn remap(dt: DataType, number: &DataType) -> DataType {
    match dt {
        DataType::Primitive(ref p) if is_ts_number(p) => number.clone(),
        DataType::List(mut l) => {
            l.ty = Box::new(remap(*l.ty, number));
            DataType::List(l)
        }
        DataType::Map(m) => DataType::Map(Map::new(
            remap(m.key_ty().clone(), number),
            remap(m.value_ty().clone(), number),
        )),
        DataType::Struct(mut s) => {
            s.fields = remap_fields(s.fields, number);
            DataType::Struct(s)
        }
        DataType::Enum(mut e) => {
            for (_, variant) in &mut e.variants {
                let fields = std::mem::replace(&mut variant.fields, Fields::Unit);
                variant.fields = remap_fields(fields, number);
            }
            DataType::Enum(e)
        }
        DataType::Tuple(mut t) => {
            t.elements = t.elements.into_iter().map(|e| remap(e, number)).collect();
            DataType::Tuple(t)
        }
        DataType::Nullable(b) => DataType::Nullable(Box::new(remap(*b, number))),
        DataType::Intersection(v) => {
            DataType::Intersection(v.into_iter().map(|e| remap(e, number)).collect())
        }
        // Types such as `Vec<T>` also carry the real definition inside the
        // reference (an inline definition), so rewrite that one too.
        DataType::Reference(Reference::Named(mut nr)) => {
            if let NamedReferenceType::Inline { dt, .. } = &mut nr.inner {
                let rewritten = remap((**dt).clone(), number);
                **dt = rewritten;
            }
            DataType::Reference(Reference::Named(nr))
        }
        other => other,
    }
}

/// Renames `serde_json::Value` to `JsonValue` to match UI conventions.
fn rename_known(named: &mut NamedDataType) {
    if named.name == "Value" && named.module_path == "serde_json" {
        named.name = "JsonValue".into();
    }
}

fn output_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/lib/generated/bindings.ts")
}

#[test]
fn export_bindings() {
    let number = ts_number();
    let types = specta::collect().map(|mut named| {
        if let Some(ty) = named.ty.take() {
            named.ty = Some(remap(ty, &number));
        }
        rename_known(&mut named);
        named
    });

    let path = output_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create output directory");
    }
    Typescript::default()
        .layout(Layout::FlatFile)
        .export_to(&path, &types, specta_serde::Format)
        .expect("export TypeScript bindings");

    assert!(path.exists(), "bindings were not generated: {path:?}");
}
