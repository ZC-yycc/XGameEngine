//! 路径寻址：`"machine.states[0].name"`。
//!
//! 语法取 JSON Pointer 的**子集**：点分层级、方括号取下标。不做 `~0` / `~1` 转义，
//! 因此字段名不允许含 `.` 和 `[`（现有组件字段名全部符合）。
//!
//! 为什么不支持任意结构写入：`set_path` 要是能写任意值，就等于"用字符串重建类型"，
//! 会绕开所有类型检查。这里只允许**写标量**；结构改动（切变体 / 增删元素）
//! 必须走 [`ReflectEnum`](super::ReflectEnum) / [`ReflectList`](super::ReflectList)
//! 的专用方法。

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
