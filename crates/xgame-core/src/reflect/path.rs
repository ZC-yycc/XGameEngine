//! 路径寻址：`"machine.states[0].name"`。
//!
//! 语法取 JSON Pointer 的**子集**：点分层级、方括号取下标。不做 `~0` / `~1` 转义，
//! 因此字段名不允许含 `.` 和 `[`（现有组件字段名全部符合）。
//!
//! 为什么不支持任意结构写入：`set_path` 要是能写任意值，就等于"用字符串重建类型"，
//! 会绕开所有类型检查。这里只允许**写标量**；结构改动（切变体 / 增删元素）
//! 必须走 [`ReflectEnum`](super::ReflectEnum) / [`ReflectList`](super::ReflectList)
//! 的专用方法。

use serde_json::Value;

use super::traits::Reflect;
use super::value::{ReflectError, ScalarValue, ValueRef, ValueRefMut};

/// 路径的一段。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Field(String),
    Index(usize),
}

/// 解析路径。空串、空分段、非数字下标都是错误（而不是静默跳过）。
fn parse(path: &str) -> Result<Vec<Seg>, ReflectError> {
    if path.is_empty() {
        return Err(ReflectError::EmptyPath);
    }
    let mut segs = Vec::new();
    for part in path.split('.') {
        if part.is_empty() {
            return Err(ReflectError::BadPath(path.to_string()));
        }
        // 一个分段可能是 `name`、`name[0]`、`name[0][2]`（多维暂不支持，只取第一个）。
        let (name, bracket) = match part.find('[') {
            Some(pos) => (&part[..pos], Some(&part[pos..])),
            None => (part, None),
        };
        if name.is_empty() {
            return Err(ReflectError::BadPath(path.to_string()));
        }
        if name.contains(']') {
            return Err(ReflectError::BadPath(path.to_string()));
        }
        segs.push(Seg::Field(name.to_string()));
        if let Some(rest) = bracket {
            let inner = rest
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .ok_or_else(|| ReflectError::BadPath(path.to_string()))?;
            let index: usize = inner
                .parse()
                .map_err(|_| ReflectError::BadPath(path.to_string()))?;
            segs.push(Seg::Index(index));
        }
    }
    Ok(segs)
}

fn no_field(type_name: &'static str, field: &str, path: &str) -> ReflectError {
    // 路径里只有最后一段是"人要看"的字段名；这里简化成整条路径都带上，
    // 因为调用方（编辑器 / MCP）本来就知道自己传了什么路径。
    let _ = path;
    ReflectError::NoSuchField {
        type_name,
        field: field.to_string(),
    }
}

fn walk<'a>(mut cur: ValueRef<'a>, segs: &[Seg], path: &str) -> Result<ValueRef<'a>, ReflectError> {
    for seg in segs {
        cur = match (cur, seg) {
            (ValueRef::Struct(s), Seg::Field(name)) => match s.get(name) {
                Some(v) => v,
                None => return Err(no_field(s.type_meta().name, name, path)),
            },
            (ValueRef::Enum(e), Seg::Field(name)) => {
                // 载荷字段直接按名字寻址：枚举体内只有载荷，没有额外的层级。
                // 变体名本身不可寻址 —— 它是 `ReflectEnum::variant()` 的结果。
                match e.payload_get(name) {
                    Some(v) => v,
                    None if e.payload_meta().is_some() => {
                        return Err(no_field(e.variant(), name, path))
                    }
                    None => return Err(ReflectError::NoneValue("payload")),
                }
            }
            (ValueRef::List(l), Seg::Index(i)) => {
                let len = l.len();
                match l.get(*i) {
                    Some(v) => v,
                    None => return Err(ReflectError::IndexOutOfRange { index: *i, len }),
                }
            }
            (ValueRef::Map(m), Seg::Field(name)) => match m.get(name) {
                Some(v) => v,
                None => return Err(no_field("map", name, path)),
            },
            (ValueRef::None, _) => return Err(ReflectError::NoneValue("Option")),
            (other, seg) => {
                let what = match seg {
                    Seg::Field(_) => "scalar",
                    Seg::Index(_) => "non-list",
                };
                let _ = other;
                return Err(ReflectError::NotTraversable(what));
            }
        };
    }
    Ok(cur)
}

fn walk_mut<'a>(
    mut cur: ValueRefMut<'a>,
    segs: &[Seg],
    path: &str,
) -> Result<ValueRefMut<'a>, ReflectError> {
    for seg in segs {
        cur = match (cur, seg) {
            (ValueRefMut::Struct(s), Seg::Field(name)) => s.get_mut_checked(name)?,
            (ValueRefMut::Enum(e), Seg::Field(name)) => {
                // 先取"有没有载荷"和变体名，再要可变引用 —— 两次借用不能重叠。
                let has_payload = e.payload_meta().is_some();
                let variant = e.variant();
                match e.payload_get_mut(name) {
                    Some(v) => v,
                    None if has_payload => return Err(no_field(variant, name, path)),
                    None => return Err(ReflectError::NoneValue("payload")),
                }
            }
            (ValueRefMut::List(l), Seg::Index(i)) => {
                let len = l.len();
                match l.get_mut(*i) {
                    Some(v) => v,
                    None => return Err(ReflectError::IndexOutOfRange { index: *i, len }),
                }
            }
            (ValueRefMut::Map(m), Seg::Field(name)) => match m.get_mut(name) {
                Some(v) => v,
                None => return Err(no_field("map", name, path)),
            },
            (ValueRefMut::None, _) => return Err(ReflectError::NoneValue("Option")),
            (_, seg) => {
                let what = match seg {
                    Seg::Field(_) => "scalar",
                    Seg::Index(_) => "non-list",
                };
                return Err(ReflectError::NotTraversable(what));
            }
        };
    }
    Ok(cur)
}

/// 按路径读一个字段。
pub fn get_path<'a>(root: &'a dyn Reflect, path: &str) -> Result<ValueRef<'a>, ReflectError> {
    let segs = parse(path)?;
    walk(ValueRef::Struct(root), &segs, path)
}

/// 按路径拿一个可写槽位。
pub fn get_path_mut<'a>(
    root: &'a mut dyn Reflect,
    path: &str,
) -> Result<ValueRefMut<'a>, ReflectError> {
    let segs = parse(path)?;
    walk_mut(ValueRefMut::Struct(root), &segs, path)
}

/// 按路径写一个标量。类型必须与目标字段完全一致（见 `ValueRefMut::set`）。
pub fn set_scalar(
    root: &mut dyn Reflect,
    path: &str,
    value: &ScalarValue,
) -> Result<(), ReflectError> {
    let mut slot = get_path_mut(root, path)?;
    slot.set(value)
}

/// 按路径用 JSON 值整体替换字段、列表项、映射项或枚举载荷字段。
///
/// 与 [`set_scalar`] 的区别：它允许 `Option` / 结构体 / 枚举 / 列表整体替换。
/// 实现按最后一段的容器形态分派到 `Reflect::set_json` / `ReflectList::set_json` /
/// `ReflectMap::set_json` / `ReflectEnum::payload_set_json`。
pub fn set_path_json(
    root: &mut dyn Reflect,
    path: &str,
    value: Value,
) -> Result<(), ReflectError> {
    let segs = parse(path)?;
    let (last, parents) = segs
        .split_last()
        .expect("parse 保证路径至少有一段");
    let mut parent = walk_mut(ValueRefMut::Struct(root), parents, path)?;
    match (&mut parent, last) {
        (ValueRefMut::Struct(s), Seg::Field(name)) => s.set_json(name, value),
        (ValueRefMut::Enum(e), Seg::Field(name)) => e.payload_set_json(name, value),
        (ValueRefMut::List(l), Seg::Index(index)) => l.set_json(*index, value),
        (ValueRefMut::Map(m), Seg::Field(key)) => m.set_json(key, value),
        (ValueRefMut::None, _) => Err(ReflectError::NoneValue("Option")),
        (_, seg) => Err(ReflectError::NotTraversable(match seg {
            Seg::Field(_) => "scalar",
            Seg::Index(_) => "non-list",
        })),
    }
}

/// 把 JSON Pointer（`/levels/0/model`）转成反射路径（`levels[0].model`）。
///
/// 只支持 MCP / `component_edit` 现有用到的子集：字段名不含 `.`、`[`、`]`，
/// `~0` / `~1` 转义会被还原，但不把转义后的名字重新解释成路径语法。
pub fn pointer_to_path(pointer: &str) -> Result<String, ReflectError> {
    if !pointer.starts_with('/') {
        return Err(ReflectError::BadPath(pointer.to_string()));
    }
    let mut out = String::new();
    for (index, raw) in pointer.split('/').skip(1).enumerate() {
        let part = raw.replace("~1", "/").replace("~0", "~");
        if part.is_empty() {
            return Err(ReflectError::BadPath(pointer.to_string()));
        }
        if index > 0 {
            out.push('.');
        }
        match part.parse::<usize>() {
            Ok(number) => {
                if out.ends_with('.') {
                    out.pop();
                }
                out.push('[');
                out.push_str(&number.to_string());
                out.push(']');
            }
            Err(_) => {
                if part.contains(['.', '[', ']']) {
                    return Err(ReflectError::BadPath(pointer.to_string()));
                }
                out.push_str(&part);
            }
        }
    }
    if out.is_empty() {
        return Err(ReflectError::BadPath(pointer.to_string()));
    }
    Ok(out)
}

/// 按 JSON Pointer 写入 JSON 值。
pub fn set_json_pointer(
    root: &mut dyn Reflect,
    pointer: &str,
    value: Value,
) -> Result<(), ReflectError> {
    let path = pointer_to_path(pointer)?;
    set_path_json(root, &path, value)
}

/// 按路径读成一行可读文本。路径不存在时返回 `None`（编辑器 / MCP 用来避免
/// 为了"显示一下"而处理一堆错误类型）。
pub fn get_display(root: &dyn Reflect, path: &str) -> Option<String> {
    get_path(root, path).ok().map(|v| v.to_display_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_splits_fields_and_indices() {
        assert_eq!(
            parse("a.b[0].c").unwrap(),
            vec![
                Seg::Field("a".into()),
                Seg::Field("b".into()),
                Seg::Index(0),
                Seg::Field("c".into()),
            ]
        );
        assert_eq!(parse("clip").unwrap(), vec![Seg::Field("clip".into())]);
    }

    #[test]
    fn parse_rejects_malformed_paths() {
        for bad in ["", ".", "a..b", "a.", "[0]", "a[x]", "a[0", "a[-1]"] {
            assert!(parse(bad).is_err(), "`{bad}` 应该被拒绝");
        }
    }
}
