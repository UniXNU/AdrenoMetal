// SPDX-License-Identifier: MIT
//! Metal library and function handles over the translator's immutable AIR inputs.

use metal2vulkan::Budget;
use metal2vulkan::error::Error;
use metal2vulkan::library::{FunctionConstantValues, Library as AirLibrary};
use metal2vulkan::metal::{FunctionType, PatchType};
use metal2vulkan::pipeline::{FunctionDescriptor, LinkedFunction};
use metal2vulkan::reflection::{Attribute, FunctionConstant, FunctionProperties};
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub struct Library(AirLibrary);

impl Library {
    pub fn from_air(bytes: &[u8]) -> Result<Self, Error> {
        AirLibrary::from_air(bytes).map(Self)
    }

    pub fn from_metallib(bytes: &[u8]) -> Result<Self, Error> {
        AirLibrary::from_metallib(bytes).map(Self)
    }

    pub fn function_names(&self) -> Vec<String> {
        self.0
            .functions()
            .map(|function| function.name.to_owned())
            .collect()
    }

    pub fn new_function_with_name(&self, name: &str) -> Result<Function, Error> {
        self.0.function(name).map(Function::new)
    }

    pub fn new_function_with_name_constant_values(
        &self,
        name: &str,
        constants: &FunctionConstantValues,
    ) -> Result<Function, Error> {
        let mut function = Function::new(self.0.function(name)?);
        function.descriptor.constants = constants.clone();
        function.descriptor.specialized = true;
        Ok(function)
    }
}

#[derive(Clone)]
pub struct Function {
    descriptor: FunctionDescriptor,
    linked_name: Option<String>,
    properties: OnceLock<Result<FunctionProperties, Error>>,
}

impl Function {
    fn new(function: metal2vulkan::library::Function) -> Self {
        Self {
            descriptor: FunctionDescriptor::new(function),
            linked_name: None,
            properties: OnceLock::new(),
        }
    }

    fn properties(&self) -> Result<&FunctionProperties, Error> {
        self.properties
            .get_or_init(|| {
                let budget = Budget::standard();
                if self.descriptor.specialized {
                    self.descriptor
                        .function
                        .specialized_properties(&self.descriptor.constants, &budget)
                } else {
                    self.descriptor.function.properties(&budget)
                }
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    pub fn function_descriptor(&self) -> FunctionDescriptor {
        self.descriptor.clone()
    }

    pub fn linked_function(&self) -> LinkedFunction {
        let linked = LinkedFunction::new(self.descriptor.function.clone())
            .with_constants(self.descriptor.constants.clone())
            .with_specialized(self.descriptor.specialized);
        match &self.linked_name {
            Some(name) => linked.with_specialized_name(name.clone()),
            None => linked,
        }
    }

    pub fn with_specialized_name(mut self, name: Option<String>) -> Self {
        self.linked_name = name;
        self
    }

    pub fn function_type(&self) -> Result<FunctionType, Error> {
        self.properties().map(FunctionProperties::function_type)
    }

    pub fn patch_type(&self) -> Result<PatchType, Error> {
        self.properties().map(FunctionProperties::patch_type)
    }

    pub fn patch_control_point_count(&self) -> Result<Option<u32>, Error> {
        self.properties()
            .map(FunctionProperties::patch_control_point_count)
    }

    pub fn stage_input_attributes(&self) -> Result<Option<Vec<Attribute>>, Error> {
        self.properties()
            .map(|p| p.stage_input_attributes().map(<[Attribute]>::to_vec))
    }

    pub fn function_constants_dictionary(
        &self,
    ) -> Result<BTreeMap<String, FunctionConstant>, Error> {
        if self.descriptor.specialized {
            Ok(BTreeMap::new())
        } else {
            self.descriptor.function.function_constants_dictionary()
        }
    }
}
