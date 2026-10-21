//! Validate compiled resources, not source spelling. The compatibility ABI has
//! scalar sampler2D bindings, one generated std140 vec4 array UBO, and only the
//! fixed vertex module's 80-byte push block. Unused declarations may disappear.
//! This is an interface admission check, not a general SPIR-V validator.
use std::collections::{HashMap, HashSet};

type Instruction<'a> = (u16, &'a [u32]);
struct Module<'a> {
    types: HashMap<u32, Instruction<'a>>,
    constants: HashMap<u32, &'a [u32]>,
    variables: Vec<&'a [u32]>,
    decorations: HashMap<(u32, u32), &'a [u32]>,
    members: HashMap<(u32, u32, u32), &'a [u32]>,
}
fn error(message: &str) -> String {
    format!("animation SPIR-V interface: {message}")
}
impl<'a> Module<'a> {
    fn read(words: &'a [u32], stage: u32) -> Result<Self, String> {
        if words.len() < 5 || words[0] != 0x0723_0203 || words[3] == 0 {
            return Err(error("invalid module header"));
        }
        let mut module = Self {
            types: HashMap::new(),
            constants: HashMap::new(),
            variables: Vec::new(),
            decorations: HashMap::new(),
            members: HashMap::new(),
        };
        let mut cursor = 5;
        let mut entries = 0;
        while cursor < words.len() {
            let length = (words[cursor] >> 16) as usize;
            let opcode = words[cursor] as u16;
            let end = cursor
                .checked_add(length)
                .filter(|end| *end <= words.len())
                .ok_or_else(|| error("truncated instruction"))?;
            if length == 0 {
                return Err(error("zero-length instruction"));
            }
            let args = &words[cursor + 1..end];
            match opcode {
                15 => {
                    if args.len() < 3 || args[0] != stage {
                        return Err(error("unexpected shader entry stage"));
                    }
                    entries += 1;
                }
                19..=38 => {
                    let id = *args.first().ok_or_else(|| error("missing type identity"))?;
                    if id == 0
                        || id >= words[3]
                        || module.types.insert(id, (opcode, args)).is_some()
                    {
                        return Err(error("invalid/duplicate type identity"));
                    }
                    // No buffers, atomic counters, physical address pointers or
                    // unknown storage classes can bypass descriptor admission.
                    if opcode == 32
                        && (args.len() != 3 || !matches!(args[1], 0 | 1 | 2 | 3 | 6 | 7 | 9))
                    {
                        return Err(error("unsupported resource pointer storage class"));
                    }
                }
                43 => {
                    if args.len() < 3 {
                        return Err(error("truncated constant"));
                    }
                    module.constants.insert(args[1], args);
                }
                59 => {
                    if args.len() < 3 {
                        return Err(error("truncated variable"));
                    }
                    module.variables.push(args);
                }
                71 => {
                    if args.len() < 2 {
                        return Err(error("truncated decoration"));
                    }
                    if matches!(args[1], 2 | 3 | 6 | 11 | 30 | 31 | 32 | 33 | 34)
                        && module
                            .decorations
                            .insert((args[0], args[1]), &args[2..])
                            .is_some()
                    {
                        return Err(error("duplicate resource decoration"));
                    }
                }
                72 => {
                    if args.len() < 3 {
                        return Err(error("truncated member decoration"));
                    }
                    if matches!(args[2], 4 | 5 | 7 | 35)
                        && module
                            .members
                            .insert((args[0], args[1], args[2]), &args[3..])
                            .is_some()
                    {
                        return Err(error("duplicate member layout decoration"));
                    }
                }
                73..=75 | 332 => {
                    return Err(error(
                        "group/id decorations are unsupported for this fixed resource ABI",
                    ))
                }
                _ => {}
            }
            cursor = end;
        }
        if entries != 1 {
            return Err(error("expected one shader entry point"));
        }
        Ok(module)
    }
    fn ty(&self, id: u32, opcode: u16) -> Option<&'a [u32]> {
        self.types
            .get(&id)
            .filter(|(op, _)| *op == opcode)
            .map(|(_, args)| *args)
    }
    fn value(&self, id: u32, decoration: u32) -> Option<u32> {
        let values = self.decorations.get(&(id, decoration))?;
        (values.len() == 1).then(|| values[0])
    }
    fn flag(&self, id: u32, decoration: u32) -> bool {
        self.decorations
            .get(&(id, decoration))
            .is_some_and(|values| values.is_empty())
    }
    fn member_value(&self, id: u32, member: u32, decoration: u32) -> Option<u32> {
        let values = self.members.get(&(id, member, decoration))?;
        (values.len() == 1).then(|| values[0])
    }
    fn float32(&self, id: u32) -> bool {
        self.ty(id, 22).is_some_and(|args| args == [id, 32])
    }
    fn vec4(&self, id: u32) -> bool {
        self.ty(id, 23)
            .is_some_and(|args| args.len() == 3 && args[2] == 4 && self.float32(args[1]))
    }
    fn vec2(&self, id: u32) -> bool {
        self.ty(id, 23)
            .is_some_and(|args| args.len() == 3 && args[2] == 2 && self.float32(args[1]))
    }
    fn scalar_2d_sampler(&self, id: u32) -> bool {
        let Some(sampled) = self.ty(id, 27).filter(|args| args.len() == 2) else {
            return false;
        };
        let Some(image) = self.ty(sampled[1], 25) else {
            return false;
        };
        // Float32, Dim2D, nondepth/nonarray/nonMS, sampled image, Unknown format.
        image.len() == 8 && self.float32(image[1]) && image[2..] == [1, 0, 0, 0, 1, 0]
    }
    fn constant_length(&self, id: u32) -> Option<usize> {
        let args = self.constants.get(&id)?;
        let int = self.ty(args[0], 21)?;
        if args.len() != 3
            || int.len() != 3
            || int[1] != 32
            || int[2] > 1
            || (int[2] == 1 && args[2] > i32::MAX as u32)
        {
            return None;
        }
        usize::try_from(args[2]).ok()
    }
    fn uniform_block(&self, id: u32, slots: usize) -> bool {
        let Some(block) = self.ty(id, 30).filter(|args| args.len() == 2) else {
            return false;
        };
        let Some(array) = self.ty(block[1], 28).filter(|args| args.len() == 3) else {
            return false;
        };
        self.flag(id, 2)
            && !self.flag(id, 3)
            && self.member_value(id, 0, 35) == Some(0)
            && self.value(block[1], 6) == Some(16)
            && self.vec4(array[1])
            && self.constant_length(array[2]) == Some(slots.max(1))
    }
    fn vertex_push(&self, id: u32) -> bool {
        let Some(block) = self.ty(id, 30).filter(|args| args.len() == 3) else {
            return false;
        };
        let Some(matrix) = self.ty(block[1], 24).filter(|args| args.len() == 3) else {
            return false;
        };
        self.flag(id, 2)
            && matrix[2] == 4
            && self.vec4(matrix[1])
            && self.vec4(block[2])
            && self.member_value(id, 0, 35) == Some(0)
            && self.member_value(id, 1, 35) == Some(64)
            && self.member_value(id, 0, 7) == Some(16)
            && self
                .members
                .get(&(id, 0, 5))
                .is_some_and(|values| values.is_empty())
            && !self.members.contains_key(&(id, 0, 4))
    }
    fn check(
        &self,
        fragment: bool,
        slots: usize,
        sampler_count: usize,
        uniform_binding: u32,
    ) -> Result<(), String> {
        let mut bindings = HashSet::new();
        let mut push = false;
        let mut color_output = false;
        let mut texture_input = false;
        for variable in &self.variables {
            let pointer = self
                .ty(variable[0], 32)
                .filter(|args| args.len() == 3)
                .ok_or_else(|| error("variable has no supported pointer type"))?;
            let storage = variable[2];
            if pointer[1] != storage {
                return Err(error("variable/pointer storage mismatch"));
            }
            match storage {
                0 | 2 => {
                    if !fragment {
                        return Err(error("fixed vertex module declares a descriptor"));
                    }
                    let set = self
                        .value(variable[1], 34)
                        .ok_or_else(|| error("resource is missing DescriptorSet"))?;
                    let binding = self
                        .value(variable[1], 33)
                        .ok_or_else(|| error("resource is missing Binding"))?;
                    if set != 0 || !bindings.insert(binding) {
                        return Err(error("extra descriptor set or duplicate binding"));
                    }
                    if storage == 0 {
                        if usize::try_from(binding)
                            .ok()
                            .is_none_or(|binding| binding >= sampler_count)
                            || !self.scalar_2d_sampler(pointer[2])
                        {
                            return Err(error(
                                "sampler descriptor does not match scalar sampler2D ABI",
                            ));
                        }
                    } else if binding != uniform_binding || !self.uniform_block(pointer[2], slots) {
                        return Err(error(
                            "uniform descriptor does not match generated std140 Data ABI",
                        ));
                    }
                }
                9 => {
                    if fragment || push || !self.vertex_push(pointer[2]) {
                        return Err(error("unexpected push constant interface"));
                    }
                    push = true;
                }
                3 if fragment => {
                    if color_output
                        || self.value(variable[1], 30) != Some(0)
                        || self.decorations.contains_key(&(variable[1], 11))
                        || self.value(variable[1], 31).unwrap_or(0) != 0
                        || self.value(variable[1], 32).unwrap_or(0) != 0
                        || !self.vec4(pointer[2])
                    {
                        return Err(error("fragment output does not match location0 vec4 ABI"));
                    }
                    color_output = true;
                }
                1 if fragment => {
                    // Builtins are supplied by the rasterizer. User varyings
                    // must match the fixed vertex shader's sole vec2 output.
                    if self.decorations.contains_key(&(variable[1], 11)) {
                        continue;
                    }
                    if texture_input
                        || self.value(variable[1], 30) != Some(0)
                        || self.value(variable[1], 31).unwrap_or(0) != 0
                        || !self.vec2(pointer[2])
                    {
                        return Err(error("fragment input does not match location0 vec2 ABI"));
                    }
                    texture_input = true;
                }
                1 | 3 | 6 | 7 => {}
                _ => return Err(error("unsupported resource variable storage class")),
            }
        }
        Ok(())
    }
}
pub(super) fn validate(
    vert: &[u32],
    frag: &[u32],
    slots: usize,
    samplers: usize,
    uniform_binding: u32,
) -> Result<(), String> {
    Module::read(vert, 0)?.check(false, 0, 0, 0)?;
    Module::read(frag, 4)?.check(true, slots, samplers, uniform_binding)
}
