use mlua::{Lua, Table, Value};

pub fn register(lua: &Lua) -> mlua::Result<()> {
    let spyweb = lua.globals().get::<Table>("spyweb").or_else(|_| {
        let t = lua.create_table()?;
        lua.globals().set("spyweb", t.clone())?;
        Ok::<Table, mlua::Error>(t)
    })?;

    spyweb.set(
        "assert_eq",
        lua.create_function(|_, (left, right, msg): (Value, Value, Option<String>)| {
            if !values_equal(&left, &right) {
                let error_msg = format!(
                    "assertion failed: `(left == right)`\n  left: `{:?}`\n right: `{:?}`{}",
                    ValueDebug(&left),
                    ValueDebug(&right),
                    msg.map(|m| format!("\nmessage: {}", m)).unwrap_or_default()
                );
                return Err(mlua::Error::RuntimeError(error_msg));
            }
            Ok(())
        })?,
    )?;

    spyweb.set(
        "assert_ne",
        lua.create_function(|_, (left, right, msg): (Value, Value, Option<String>)| {
            if values_equal(&left, &right) {
                let error_msg = format!(
                    "assertion failed: `(left != right)`\n  left: `{:?}`\n right: `{:?}`{}",
                    ValueDebug(&left),
                    ValueDebug(&right),
                    msg.map(|m| format!("\nmessage: {}", m)).unwrap_or_default()
                );
                return Err(mlua::Error::RuntimeError(error_msg));
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Nil, Value::Nil) => true,
        (Value::Boolean(a), Value::Boolean(b)) => a == b,
        (Value::Integer(a), Value::Integer(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Table(a), Value::Table(b)) => {
            if a.len().ok() != b.len().ok() {
                return false;
            }
            let pairs_a = match a
                .clone()
                .pairs::<Value, Value>()
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(p) => p,
                Err(_) => return false,
            };

            for (k, v_a) in pairs_a {
                let v_b: Value = match b.get(k) {
                    Ok(v) => v,
                    Err(_) => return false,
                };
                if !values_equal(&v_a, &v_b) {
                    return false;
                }
            }

            let pairs_b = match b
                .clone()
                .pairs::<Value, Value>()
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(p) => p,
                Err(_) => return false,
            };
            for (k, _) in pairs_b {
                if !a.contains_key(k).unwrap_or(false) {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}

struct ValueDebug<'a>(&'a Value);

impl std::fmt::Debug for ValueDebug<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Value::Nil => write!(f, "nil"),
            Value::Boolean(b) => write!(f, "{}", b),
            Value::Integer(i) => write!(f, "{}", i),
            Value::Number(n) => write!(f, "{}", n),
            Value::String(s) => write!(f, "{:?}", s.to_string_lossy()),
            Value::Table(_) => write!(f, "<table>"),
            Value::Function(_) => write!(f, "<function>"),
            Value::Thread(_) => write!(f, "<thread>"),
            Value::UserData(_) => write!(f, "<userdata>"),
            Value::LightUserData(_) => write!(f, "<lightuserdata>"),
            Value::Error(e) => write!(f, "<error: {}>", e),
            _ => write!(f, "<unknown>"),
        }
    }
}
