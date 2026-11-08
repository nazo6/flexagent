//! `fxg-protocol` の型から UI 用 TypeScript 型定義
//! (`ui/src/lib/generated/bindings.ts`) を生成する。
//!
//! - 生成対象は `#[specta(collect)]` を付与した型 (およびそれらが参照する型)。
//! - `i64` / `u64` などの BigInt 系整数は JSON 上は数値であり、UI 側で
//!   `bigint` 変換を挟まないため `number` として出力する (specta は既定で
//!   精度損失を避けるためエラーにするので、明示的に `Number` へ置換する)。
//! - `serde_json::Value` は UI 側の慣習に合わせて `JsonValue` として出力する。
//!
//! 実行: `cargo test -p fxg-protocol export_bindings` (または `mise run types`)

use std::path::PathBuf;

use specta::Type;
use specta::Types;
use specta::datatype::{
    DataType, Fields, Map, NamedDataType, NamedReferenceType, Primitive, Reference,
};
use specta_typescript::{Layout, Typescript};

/// TS の `number` として出力する整数型。
///
/// specta は精度損失を避けるため既定で BigInt 系整数を `bigint` として扱い、
/// エラーにする。また浮動小数は JSON で `NaN` / `Infinity` を表現できないため
/// `number | null` として出力する。本プロトコルでは JSON 上はどちらも数値であり
/// UI 側で変換を挟まないため、すべて `number` として出力する。
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

/// `specta_typescript::Number` (TS の `number`) を表す `DataType`。
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

/// BigInt 系整数を `number` へ再帰的に置換する。
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
        // `Vec<T>` などは参照の内側 (インライン定義) に実体を持つため、
        // そちらも書き換える。
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

/// `serde_json::Value` を UI 側の慣習 (`JsonValue`) に合わせて改名する。
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
