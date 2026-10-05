//! Namespace-local source imports, using the VM's dict-backed module scopes.
use crate::{
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    heap::{DropWithContext, HeapData, HeapId},
    intern::CompileInterns,
    types::{Dict, Module, str::allocate_string},
    value::Value,
};
use std::sync::Arc;

fn root<'a>(name: &str, vm: &'a VM<'_>) -> Option<&'a Value> {
    let name = vm.interns.get_string_id_by_name(name)?;
    let slot = vm.global_names.get(name)?;
    vm.globals.get(slot.index())
}
fn dict_id(value: &Value, vm: &VM<'_>) -> Option<HeapId> {
    if let Value::Ref(id) = value
        && matches!(vm.heap.get(*id), HeapData::Dict(_))
    {
        Some(*id)
    } else {
        None
    }
}
fn lookup<'a>(id: HeapId, name: &str, vm: &'a VM<'_>) -> Option<&'a Value> {
    let HeapData::Dict(dict) = vm.heap.get(id) else {
        return None;
    };
    dict.get_by_str(name, vm.heap, vm.interns)
}
fn insert(id: HeapId, name: &str, value: Value, vm: &mut VM<'_>) -> RunResult<()> {
    let key = allocate_string(name.to_owned(), vm.heap);
    let old = vm
        .heap
        .read_as::<Dict>(id)
        .expect("module dictionary")
        .set(key, value, vm)?;
    old.drop_with(vm);
    Ok(())
}

pub(crate) fn load(requested: &str, vm: &mut VM<'_>) -> RunResult<Option<CallResult>> {
    let Some(sources) = root("__state_sources", vm).and_then(|v| dict_id(v, vm)) else {
        return Ok(None);
    };
    let cache = root("__state_modules", vm)
        .and_then(|v| dict_id(v, vm))
        .ok_or_else(|| ExcType::type_error("module cache must be a dictionary"))?;
    let relative = requested.chars().take_while(|c| *c == '.').count();
    let name = if relative > 0 {
        let package = vm.import_package().filter(|p| !p.is_empty()).ok_or_else(|| {
            SimpleException::new_msg(
                ExcType::ImportError,
                "attempted relative import with no known parent package",
            )
        })?;
        let mut parts: Vec<_> = package.split('.').collect();
        if relative > parts.len() {
            return Err(SimpleException::new_msg(
                ExcType::ImportError,
                "attempted relative import beyond top-level package",
            )
            .into());
        }
        parts.truncate(parts.len() + 1 - relative);
        let tail = &requested[relative..];
        if !tail.is_empty() {
            parts.push(tail);
        }
        parts.join(".")
    } else {
        requested.to_owned()
    };
    let root_path = root("__state_import_root", vm)
        .and_then(|v| v.to_str(vm).ok())
        .unwrap_or("");
    let candidates = if relative == 0 && !root_path.is_empty() {
        vec![format!("{root_path}.{name}"), name.clone()]
    } else {
        vec![name.clone()]
    };
    let Some((qualified, path, source, is_package)) = candidates.iter().find_map(|qualified| {
        let base = format!("/{}", qualified.replace('.', "/"));
        for (path, is_package) in [(format!("{base}/__init__.py"), true), (format!("{base}.py"), false)] {
            if let Some(value) = lookup(sources, &path, vm) {
                if let Ok(source) = value.to_str(vm) {
                    return Some((qualified.clone(), path, source.to_owned(), is_package));
                }
            }
        }
        if let HeapData::Dict(files) = vm.heap.get(sources) {
            if files
                .into_iter()
                .any(|(key, _)| key.to_str(vm).is_ok_and(|path| path.starts_with(&format!("{base}/"))))
            {
                return Some((qualified.clone(), format!("{base}/__init__.py"), String::new(), true));
            }
        }
        None
    }) else {
        return Ok(None);
    };
    if let Some(cached) = lookup(cache, &qualified, vm) {
        return Ok(Some(CallResult::Value(cached.clone_with_heap(vm.heap))));
    }
    if let Some((parent, _)) = qualified.rsplit_once('.') {
        if lookup(cache, parent, vm).is_none() {
            // Initialize the parent before caching the child: package __init__
            // may itself import this child and must see its complete globals.
            let globals = vm.heap.allocate(HeapData::Dict(Dict::new()));
            vm.heap.inc_ref(cache);
            insert(globals, "__state_modules", Value::Ref(cache), vm)?;
            let source = Arc::from(format!("import {parent}\nimport {qualified} as __state_module__\n"));
            let result = super::eval_exec::source_module(source, &qualified, globals, vm);
            Value::Ref(globals).drop_with(vm);
            return result.map(Some);
        }
    }
    let globals = vm.heap.allocate(HeapData::Dict(Dict::new()));
    // The module owns its globals; that namespace is shared by its functions.
    let mut overlay = CompileInterns::new(vm.interns);
    let name_id = overlay.intern(&qualified);
    overlay.commit();
    let module = vm
        .heap
        .allocate(HeapData::Module(Box::new(Module::from_globals(name_id, globals))));
    vm.heap.inc_ref(module);
    insert(cache, &qualified, Value::Ref(module), vm)?;
    insert(globals, "__state_module__", Value::Ref(module), vm)?;
    vm.heap.inc_ref(cache);
    insert(globals, "__state_modules", Value::Ref(cache), vm)?;
    let package = if is_package {
        &qualified
    } else {
        qualified.rsplit_once('.').map_or("", |(parent, _)| parent)
    };
    for (key, text) in [
        ("__name__", qualified.as_str()),
        ("__package__", package),
        ("__file__", path.as_str()),
    ] {
        let value = allocate_string(text.to_owned(), vm.heap);
        insert(globals, key, value, vm)?;
    }
    if is_package {
        let directory = path.trim_end_matches("/__init__.py");
        let value = allocate_string(directory.to_owned(), vm.heap);
        let list = vm.heap.allocate(HeapData::List(crate::types::List::new(vec![value])));
        insert(globals, "__path__", Value::Ref(list), vm)?;
    }
    for name in [
        "mcp",
        "call",
        "db_query",
        "db_execute",
        "db_inspect",
        "read_text",
        "write_text",
    ] {
        if let Some(value) = root(name, vm) {
            let value = value.clone_with_heap(vm.heap);
            insert(globals, name, value, vm)?;
        }
    }
    let source = if let Some((parent, child)) = qualified.rsplit_once('.') {
        format!("import {parent} as __state_parent__\n__state_parent__.{child} = __state_module__\n{source}")
    } else {
        source
    };
    match super::eval_exec::source_module(Arc::from(source), &qualified, globals, vm) {
        Ok(result) => Ok(Some(result)),
        Err(error) => {
            let key = allocate_string(qualified, vm.heap);
            let removed = vm.heap.read_as::<Dict>(cache).expect("module cache").pop(&key, vm)?;
            key.drop_with(vm);
            removed.drop_with(vm);
            Err(error)
        }
    }
}

/// Import __all__, or public module attributes when __all__ is absent.
pub(crate) fn import_all(module: &Value, vm: &mut VM<'_>) -> RunResult<CallResult> {
    use crate::types::py_trait::PyTrait;
    let Value::Ref(id) = module else {
        return Err(ExcType::type_error("import * requires a module"));
    };
    let HeapData::Module(module) = vm.heap.get(*id) else {
        return Err(ExcType::type_error("import * requires a module"));
    };
    let name = module
        .globals()
        .and_then(|id| lookup(id, "__name__", vm))
        .and_then(|v| v.to_str(vm).ok())
        .unwrap_or_else(|| vm.interns.get_str(module.name()))
        .to_owned();
    let attrs = match module.globals() {
        Some(id) => {
            let HeapData::Dict(dict) = vm.heap.get(id) else {
                unreachable!()
            };
            dict
        }
        None => module.attrs(),
    };
    let all = attrs
        .get_by_str("__all__", vm.heap, vm.interns)
        .map(|v| v.clone_with_heap(vm.heap));
    let mut names = Vec::new();
    if let Some(all) = all {
        let iterator = all.py_iter(vm);
        all.drop_with(vm);
        let iterator = iterator?;
        let Value::Ref(id) = iterator else { unreachable!() };
        loop {
            let next = vm.heap.read(id).py_next(vm)?;
            let Some(value) = next else {
                break;
            };
            let text = value.to_str(vm).map(str::to_owned);
            value.drop_with(vm);
            names.push(text?);
        }
        iterator.drop_with(vm);
    } else {
        for (key, _) in attrs {
            if let Ok(text) = key.to_str(vm)
                && !text.starts_with('_')
            {
                names.push(text.to_owned());
            }
        }
    }
    if names.is_empty() {
        return Ok(CallResult::Value(Value::None));
    }
    super::eval_exec::exec_source(Arc::from(format!("from {name} import {}\n", names.join(", "))), vm)
}
