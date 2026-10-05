//! Python module type for representing imported modules.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, HeapId, HeapItem, HeapRead},
    intern::{Interns, StaticStrings, StringId},
    types::Dict,
    value::{EitherStr, Value},
};

/// A Python module with a name and attribute dictionary.
///
/// Modules in Monty are simplified compared to CPython - they just have a name
/// and a dictionary of attributes. This is sufficient for built-in modules like
/// `sys` and `typing` where we control the available attributes.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Module {
    /// The module name (e.g., "sys", "typing").
    name: StringId,
    /// The module's attributes (e.g., `version`, `platform` for `sys`).
    attrs: Dict,
    globals: Option<HeapId>,
}

impl Module {
    /// Creates a new module with an empty attributes dictionary.
    ///
    /// Attribute names are interned lazily when the module materializes them.
    pub fn new(name: StaticStrings, interns: &Interns) -> Self {
        Self {
            name: interns.intern_static(name),
            attrs: Dict::new(),
            globals: None,
        }
    }

    pub(crate) fn from_globals(name: StringId, globals: HeapId) -> Self {
        Self {
            name,
            attrs: Dict::new(),
            globals: Some(globals),
        }
    }
    pub(crate) fn globals(&self) -> Option<HeapId> {
        self.globals
    }
    fn attribute<'a>(&'a self, name: &str, vm: &'a VM<'_>) -> Option<&'a Value> {
        match self.globals {
            Some(id) => match vm.heap.get(id) {
                crate::heap::HeapData::Dict(dict) => dict.get_by_str(name, vm.heap, vm.interns),
                _ => None,
            },
            None => self.attrs.get_by_str(name, vm.heap, vm.interns),
        }
    }
    /// Returns the module's name StringId.
    pub fn name(&self) -> StringId {
        self.name
    }

    /// Returns a reference to the module's attribute dictionary.
    pub fn attrs(&self) -> &Dict {
        &self.attrs
    }

    /// Sets an attribute in the module's dictionary.
    ///
    /// Attribute names are interned lazily when the module materializes them.
    pub fn set_attr(&mut self, name: StaticStrings, value: Value, vm: &mut VM<'_>) {
        let key = Value::InternString(vm.interns.intern_static(name));
        // Module construction is infallible (`StandardLib::create`,
        // `VM::load_module`), so this insert must not be able to fail: skipping
        // the growth preflight leaves hashing, and `InternString` always hashes.
        self.attrs
            .set_without_growth_check(key, value, vm)
            .expect("module attribute keys are interned, so hashing cannot fail");
    }

    /// Returns whether this module has any heap references in its attributes.
    pub fn has_refs(&self) -> bool {
        self.globals.is_some() || self.attrs.has_refs()
    }

    /// Collects child HeapIds for reference counting.
    pub fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.attrs.py_dec_ref_ids(stack);
        if let Some(id) = self.globals.take() {
            stack.push(id);
        }
    }
}

impl<'h> HeapRead<'h, Module> {
    /// Gets an attribute by string ID for the `py_getattr` trait method.
    ///
    /// Returns the attribute value if found, or `None` if the attribute doesn't exist.
    /// For `Property` values, invokes the property getter rather than returning
    /// the Property itself - this implements Python's descriptor protocol.
    pub fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> Option<CallResult> {
        if attr.as_str(vm.interns) == "__dict__" {
            if let Some(id) = self.get(vm.heap).globals {
                vm.heap.inc_ref(id);
                return Some(CallResult::Value(Value::Ref(id)));
            }
        }
        let value = self.get(vm.heap).attribute(attr.as_str(vm.interns), vm)?;

        // If the value is a Property, invoke its getter to compute the actual value
        if let Value::Property(prop) = *value {
            Some(prop.get())
        } else {
            Some(CallResult::Value(value.clone_with_heap(vm)))
        }
    }

    pub fn py_set_attr(&mut self, name: &EitherStr, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        if let Some(id) = self.get(vm.heap).globals {
            let key = match name {
                EitherStr::Interned(id) => Value::InternString(*id),
                EitherStr::Heap(text) => crate::types::str::allocate_string(text.clone(), vm.heap),
            };
            let old = vm
                .heap
                .read_as::<Dict>(id)
                .expect("module globals dictionary")
                .set(key, value, vm)?;
            use crate::heap::DropWithContext;
            old.drop_with(vm);
            Ok(())
        } else {
            value.drop_with(vm);
            Err(ExcType::type_error("built-in module attributes are read-only"))
        }
    }

    /// Calls an attribute as a function on this module.
    ///
    /// Modules don't have methods - they have callable attributes. This looks up
    /// the attribute and calls it if it's a `ModuleFunction`.
    ///
    /// Returns `CallResult` because module functions may need OS operations
    /// (e.g., `os.getenv()`) that require host involvement.
    pub fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let mut args_guard = DropGuard::new(args, vm);
        let vm = args_guard.ctx();

        let attr_str = match attr {
            EitherStr::Interned(id) => vm.interns.get_str(*id),
            EitherStr::Heap(s) => {
                return Err(ExcType::attribute_error_module(
                    vm.interns.get_str(self.get(vm.heap).name),
                    s,
                ));
            }
        };

        match self.get(vm.heap).attribute(attr_str, vm) {
            Some(value) => {
                let value = value.clone_with_heap(vm);
                let (args, vm) = args_guard.into_parts();
                defer_drop!(value, vm);
                vm.call_function(value, args)
            }
            None => Err(ExcType::attribute_error_module(
                vm.interns.get_str(self.get(vm.heap).name),
                attr.as_str(vm.interns),
            )),
        }
    }
}

impl HeapItem for Module {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.attrs.py_dec_ref_ids(stack);
        if let Some(id) = self.globals.take() {
            stack.push(id);
        }
    }
}
