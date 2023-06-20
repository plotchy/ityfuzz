use crate::generic_vm::vm_executor::{ExecutionResult, GenericVM, MAP_SIZE};
use crate::generic_vm::vm_state::VMStateT;
use crate::input::VMInputT;
use crate::r#move::input::{MoveFunctionInput, MoveFunctionInputT, StructAbilities};
use crate::r#move::types::MoveOutput;
use crate::r#move::vm_state::MoveVMState;
use crate::state_input::StagedVMState;

use move_binary_format::access::ModuleAccess;

use move_binary_format::CompiledModule;
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::Identifier;
use move_core_types::language_storage::ModuleId;

use move_vm_runtime::interpreter::{CallStack, DummyTracer, ExitCode, Frame, Interpreter, ItyFuzzTracer, Stack};
use move_vm_runtime::loader;
use move_vm_runtime::loader::BinaryType::Module;
use move_vm_runtime::loader::{Function, Loader, ModuleCache, Resolver};
use move_vm_runtime::native_functions::NativeFunctions;
use move_vm_types::gas::UnmeteredGasMeter;
use move_vm_types::values;
use move_vm_types::values::{Locals, Reference, StructRef, Value, ValueImpl, VMValueCast};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use libafl::state::HasMetadata;
use move_binary_format::errors::VMResult;
use move_binary_format::file_format::Bytecode;
use move_core_types::u256;

pub static mut MOVE_COV_MAP: [u8; MAP_SIZE] = [0u8; MAP_SIZE];
pub static mut MOVE_CMP_MAP: [u128; MAP_SIZE] = [0; MAP_SIZE];
pub static mut MOVE_READ_MAP: [bool; MAP_SIZE] = [false; MAP_SIZE];
pub static mut MOVE_WRITE_MAP: [u8; MAP_SIZE] = [0u8; MAP_SIZE];
pub static mut MOVE_STATE_CHANGED: bool = false;
pub struct MoveVM<I, S> {
    // for comm with move_vm
    pub functions: HashMap<ModuleId, HashMap<Identifier, Arc<Function>>>,
    pub loader: Loader,
    _phantom: std::marker::PhantomData<(I, S)>,
}

impl<I, S> MoveVM<I, S> {
    pub fn new() -> Self {
        let functions = HashMap::new();
        Self {
            functions,
            loader: Loader::new(NativeFunctions::new(vec![]).unwrap(), Default::default()),
            _phantom: Default::default(),
        }
    }

    pub fn get_natives(&self) -> NativeFunctions {
        NativeFunctions {
            0: Default::default(),
        }
    }
}

pub struct MoveVMTracer {

}

impl ItyFuzzTracer for MoveVMTracer {
    fn on_step(&mut self, interpreter: &Interpreter, frame: &Frame, pc: u16, instruction: &Bytecode) {
        macro_rules! fast_peek_back {
            ($interp: expr) => { &$interp.operand_stack.value[$interp.operand_stack.value.len() - 1] };
            ($interp: expr, $kth: expr) => { &$interp.operand_stack.value[$interp.operand_stack.value.len() - $kth] };
        }
        macro_rules! distance {
            ($cond:expr, $l:expr, $v:expr) => {
                if !($cond) {
                    if *$l > *$v {
                        (*$l - *$v) as u128
                    } else {
                        (*$v - *$l) as u128
                    }
                } else {
                    0u128
                }
            };
        }

        match instruction {
            // COV MAP
            Bytecode::BrTrue(offset) => {
                if let Value(ValueImpl::Bool(b)) = fast_peek_back!(interpreter) {
                    let next_pc = if *b { *offset } else { pc + 1 };
                    let map_offset = next_pc as usize % MAP_SIZE;
                    unsafe {MOVE_COV_MAP[map_offset] = (MOVE_COV_MAP[map_offset] + 1) % 255;}
                } else {
                    unreachable!("brtrue with non-bool value")
                }
            }
            Bytecode::BrFalse(offset) => {
                if let Value(ValueImpl::Bool(b)) = fast_peek_back!(interpreter) {
                    let next_pc = if !*b { *offset } else { pc + 1 };
                    let map_offset = next_pc as usize % MAP_SIZE;
                    unsafe {MOVE_COV_MAP[map_offset] = (MOVE_COV_MAP[map_offset] + 1) % 255;}
                } else {
                    unreachable!("brfalse with non-bool value")
                }
            }


            // CMP MAP
            Bytecode::Eq => {
                let distance = match (fast_peek_back!(interpreter), fast_peek_back!(interpreter, 2)) {
                    (Value(ValueImpl::U8(l)), Value(ValueImpl::U8(r))) => distance!(*l==*r, l, r),
                    (Value(ValueImpl::U16(l)), Value(ValueImpl::U16(r))) => distance!(*l==*r, l, r),
                    (Value(ValueImpl::U32(l)), Value(ValueImpl::U32(r))) => distance!(*l==*r, l, r),
                    (Value(ValueImpl::U64(l)), Value(ValueImpl::U64(r))) => distance!(*l==*r, l, r),
                    (Value(ValueImpl::U128(l)), Value(ValueImpl::U128(r))) => distance!(*l==*r, l, r),
                    (Value(ValueImpl::U256(l)), Value(ValueImpl::U256(r))) => distance!(*l==*r, &l.unchecked_as_u128(), &r.unchecked_as_u128()),
                    (Value(ValueImpl::Bool(l)), Value(ValueImpl::Bool(r))) => if l == r { 0 } else { 1 },
                    _ => u128::MAX
                };

                let map_offset = pc as usize % MAP_SIZE;
                if unsafe { MOVE_CMP_MAP[map_offset] > distance } {
                    unsafe { MOVE_CMP_MAP[map_offset] = distance; }
                }
            }
            Bytecode::Neq => {}
            Bytecode::Lt | Bytecode::Le => {
                let distance = match (fast_peek_back!(interpreter), fast_peek_back!(interpreter, 2)) {
                    (Value(ValueImpl::U8(l)), Value(ValueImpl::U8(r))) => distance!(*l <= *r, l, r),
                    (Value(ValueImpl::U16(l)), Value(ValueImpl::U16(r))) => distance!(*l <= *r, l, r),
                    (Value(ValueImpl::U32(l)), Value(ValueImpl::U32(r))) => distance!(*l <= *r, l, r),
                    (Value(ValueImpl::U64(l)), Value(ValueImpl::U64(r))) => distance!(*l <= *r, l, r),
                    (Value(ValueImpl::U128(l)), Value(ValueImpl::U128(r))) => distance!(*l <= *r, l, r),
                    (Value(ValueImpl::U256(l)), Value(ValueImpl::U256(r))) => distance!(*l <= *r, &l.unchecked_as_u128(), &r.unchecked_as_u128()),
                    _ => u128::MAX
                };

                let map_offset = pc as usize % MAP_SIZE;
                if unsafe { MOVE_CMP_MAP[map_offset] > distance } {
                    unsafe { MOVE_CMP_MAP[map_offset] = distance; }
                }
            }
            Bytecode::Gt | Bytecode::Ge => {
                let distance = match (fast_peek_back!(interpreter), fast_peek_back!(interpreter, 2)) {
                    (Value(ValueImpl::U8(l)), Value(ValueImpl::U8(r))) => distance!(*l >= *r, l, r),
                    (Value(ValueImpl::U16(l)), Value(ValueImpl::U16(r))) => distance!(*l >= *r, l, r),
                    (Value(ValueImpl::U32(l)), Value(ValueImpl::U32(r))) => distance!(*l >= *r, l, r),
                    (Value(ValueImpl::U64(l)), Value(ValueImpl::U64(r))) => distance!(*l >= *r, l, r),
                    (Value(ValueImpl::U128(l)), Value(ValueImpl::U128(r))) => distance!(*l >= *r, l, r),
                    (Value(ValueImpl::U256(l)), Value(ValueImpl::U256(r))) => distance!(*l >= *r, &l.unchecked_as_u128(), &r.unchecked_as_u128()),
                    _ => u128::MAX
                };

                let map_offset = pc as usize % MAP_SIZE;
                if unsafe { MOVE_CMP_MAP[map_offset] > distance } {
                    unsafe { MOVE_CMP_MAP[map_offset] = distance; }
                }
            }

            // RW MAP & Onchain stuffs
            Bytecode::MutBorrowGlobal(sd_idx) |
            Bytecode::ImmBorrowGlobal(sd_idx) |
            Bytecode::Exists(sd_idx) |
            Bytecode::MoveFrom(sd_idx) => unsafe {
                let addr_off = if let Value(ValueImpl::Address(addr)) = fast_peek_back!(interpreter) {
                    u128::from_le_bytes(
                        addr.as_slice()[addr.len() - 16..]
                            .try_into()
                            .expect("slice with incorrect length"),
                    )
                } else {
                    unreachable!("borrow_global with non-address value")
                };
                let offset = sd_idx.0 as u16;
                let map_offset = (addr_off.unchecked_add(offset as u128) % (MAP_SIZE as u128)) as usize;
                if !MOVE_READ_MAP[map_offset] {
                    MOVE_READ_MAP[map_offset] = true;
                }
            }
            Bytecode::MutBorrowGlobalGeneric(sd_idx) |
            Bytecode::ImmBorrowGlobalGeneric(sd_idx) |
            Bytecode::ExistsGeneric(sd_idx) |
            Bytecode::MoveFromGeneric(sd_idx) => unsafe {
                let addr_off = if let Value(ValueImpl::Address(addr)) = fast_peek_back!(interpreter) {
                    u128::from_le_bytes(
                        addr.as_slice()[addr.len() - 16..]
                            .try_into()
                            .expect("slice with incorrect length"),
                    )
                } else {
                    unreachable!("borrow_global with non-address value")
                };
                let offset = sd_idx.0 as u16;
                let map_offset = (addr_off.unchecked_add(offset as u128) % (MAP_SIZE as u128)) as usize;
                if !MOVE_READ_MAP[map_offset] {
                    MOVE_READ_MAP[map_offset] = true;
                }
            }
            Bytecode::MoveTo(sd_idx) => unsafe {
                let addr_struct: StructRef = fast_peek_back!(interpreter, 2).clone().cast().unwrap();
                let addr = addr_struct.borrow_field(0).unwrap()
                    .value_as::<Reference>().unwrap()
                    .read_ref().unwrap()
                    .value_as::<AccountAddress>().unwrap();


                let addr_off = u128::from_le_bytes(
                    addr.as_slice()[addr.len() - 16..]
                        .try_into()
                        .expect("slice with incorrect length"),
                );
                let offset = sd_idx.0 as u16;
                let map_offset = (addr_off.unchecked_add(offset as u128) % (MAP_SIZE as u128)) as usize;
                if MOVE_WRITE_MAP[map_offset] == 0 {
                    MOVE_WRITE_MAP[map_offset] = 1;
                }
            }
            Bytecode::MoveToGeneric(sd_idx) => unsafe {
                let addr_struct: StructRef = fast_peek_back!(interpreter, 2).clone().cast().unwrap();
                let addr = addr_struct.borrow_field(0).unwrap()
                    .value_as::<Reference>().unwrap()
                    .read_ref().unwrap()
                    .value_as::<AccountAddress>().unwrap();


                let addr_off = u128::from_le_bytes(
                    addr.as_slice()[addr.len() - 16..]
                        .try_into()
                        .expect("slice with incorrect length"),
                );
                let offset = sd_idx.0 as u16;
                let map_offset = (addr_off.unchecked_add(offset as u128) % (MAP_SIZE as u128)) as usize;
                if MOVE_WRITE_MAP[map_offset] == 0 {
                    MOVE_WRITE_MAP[map_offset] = 1;
                }
            }


            // Onchain stuffs
            Bytecode::Call(_) => {}
            Bytecode::CallGeneric(_) => {}
            _ => {}
        }
    }
}


impl<I, S>
    GenericVM<
        MoveVMState,
        CompiledModule,
        MoveFunctionInput,
        ModuleId,
        AccountAddress,
        u128,
        MoveOutput,
        I,
        S,
    > for MoveVM<I, S>
where
    I: VMInputT<MoveVMState, ModuleId, AccountAddress> + MoveFunctionInputT,
    S: HasMetadata,
{
    fn deploy(
        &mut self,
        module: CompiledModule,
        _constructor_args: Option<MoveFunctionInput>,
        _deployed_address: AccountAddress,
        _state: &mut S,
    ) -> Option<AccountAddress> {
        let func_off = self.loader.module_cache.read().functions.len();
        let module_name = module.name().to_owned();
        let deployed_module_idx = module.self_id();
        self.loader.module_cache.write().insert(&self.get_natives(),
                                                deployed_module_idx.clone(),
                                                module).expect("internal deploy error");
        for f in &self.loader.module_cache.read().functions[func_off..] {
            println!("deployed function: {:?}@{}({:?}) returns {:?}", deployed_module_idx, f.name.as_str(), f.parameter_types, f.return_types());
            self.functions
                .entry(deployed_module_idx.clone())
                .or_insert_with(HashMap::new)
                .insert(f.name.to_owned(), f.clone());
        }

        println!("deployed structs: {:?}", self.loader.module_cache.read().structs);
        println!("module cache: {:?}  {:?}",
                 self.loader.module_cache.read().modules.binaries,
                 self.loader.module_cache.read().modules.id_map
        );
        Some(deployed_module_idx.address().clone())
    }


    fn fast_static_call(
        &mut self,
        _data: &Vec<(AccountAddress, MoveFunctionInput)>,
        _vm_state: &MoveVMState,
        _state: &mut S,
    ) -> Vec<MoveOutput>
    where
        MoveVMState: VMStateT,
        AccountAddress: Serialize + DeserializeOwned + Debug,
        ModuleId: Serialize + DeserializeOwned + Debug,
        MoveOutput: Default,
    {
        todo!()
    }

    fn execute(
        &mut self,
        input: &I,
        _state: &mut S,
    ) -> ExecutionResult<ModuleId, AccountAddress, MoveVMState, MoveOutput>
    where
        MoveVMState: VMStateT,
    {
        let initial_function = self
            .functions
            .get(&input.module_id())
            .unwrap()
            .get(input.function_name())
            .unwrap();

        println!("running {:?} {:?}", initial_function.name.as_str(), initial_function.scope);


        // setup interpreter
        let mut interp = Interpreter {
            operand_stack: Stack::new(),
            call_stack: CallStack::new(),
            paranoid_type_checks: false,
        };

        let mut state = input.get_state().clone();
        unsafe {
            MOVE_STATE_CHANGED = false;
        }

        // set up initial frame
        let mut current_frame = {
            let mut locals = Locals::new(initial_function.local_count());
            for (i, value) in input.args().into_iter().enumerate() {
                locals.store_loc(i, value.clone().value).unwrap();
            }
            Frame {
                pc: 0,
                locals,
                function: initial_function.clone(),
                ty_args: vec![],
            }
        };

        let mut call_stack = vec![];
        let mut reverted = false;
        loop {
            let resolver = current_frame.resolver(&self.loader);
            let ret =
                current_frame.execute_code(&resolver, &mut interp, &mut state, &mut UnmeteredGasMeter, &mut MoveVMTracer{});
            println!("{:?}", ret);

            if ret.is_err() {
                reverted = true;
                break;
            }

            match ret.unwrap() {
                ExitCode::Return => {
                    match call_stack.pop() {
                        Some(frame) => {
                            current_frame = frame;
                            current_frame.pc += 1;
                        }
                        None => {
                            break;
                        }
                    }
                }
                ExitCode::Call(fh_idx) => {
                    let func = resolver.function_from_handle(fh_idx);
                    let argc = func.local_count();
                    let mut locals = Locals::new(argc);
                    for i in 0..argc {
                        locals.store_loc(argc - i - 1, interp.operand_stack.pop().unwrap()).unwrap();
                    }
                    println!("locals: {:?}", locals);
                    // todo: handle native here
                    if func.is_native() {
                        todo!("native function call")
                    }
                    call_stack.push(current_frame);
                    current_frame = Frame {
                        pc: 0,
                        locals,
                        function: func.clone(),
                        ty_args: vec![],
                    };
                }
                ExitCode::CallGeneric(fh_idx) => {
                    let ty_args = resolver
                        .instantiate_generic_function(fh_idx, &current_frame.ty_args).unwrap();
                    let func = resolver.function_from_instantiation(fh_idx);

                    let argc = func.local_count();
                    let mut locals = Locals::new(argc);
                    for i in 0..argc {
                        locals.store_loc(argc - i - 1, interp.operand_stack.pop().unwrap()).unwrap();
                    }

                    // todo: handle native here
                    if func.is_native() {
                        todo!("native function call")
                    }
                    call_stack.push(current_frame);
                    current_frame = Frame {
                        pc: 0,
                        locals,
                        function: func.clone(),
                        ty_args,
                    };
                }
            }
        }

        let resolver = current_frame.resolver(&self.loader);


        let mut out: MoveOutput = MoveOutput { vars: vec![] };

        println!("{:?}", interp.operand_stack.value);

        for (v, t) in interp.operand_stack.value.iter().zip(
            initial_function
                .return_types()
                .iter()
        ) {
            let abilities = resolver.loader.abilities(t).expect("unknown type");
            state.add_new_value(v.clone(), t, abilities.has_drop());

            if !_state.has_metadata::<StructAbilities>() {
                _state.metadata_mut().insert(StructAbilities::new());
            }

            _state.metadata_mut().get_mut::<StructAbilities>().unwrap().set_ability(
                t.clone(),
                abilities,
            );

            out.vars.push((t.clone(), v.clone()));
            println!("val: {:?} {:?}", v, resolver.loader.type_to_type_tag(t));
        }
        ExecutionResult {
            new_state: StagedVMState::new_with_state(state),
            output: out,
            reverted,
            additional_info: None
        }
    }

    fn get_jmp(&self) -> &'static mut [u8; MAP_SIZE] {
        unsafe { &mut MOVE_COV_MAP }
    }

    fn get_read(&self) -> &'static mut [bool; MAP_SIZE] {
        unsafe { &mut MOVE_READ_MAP }
    }

    fn get_write(&self) -> &'static mut [u8; MAP_SIZE] {
        unsafe { &mut MOVE_WRITE_MAP }
    }

    fn get_cmp(&self) -> &'static mut [u128; MAP_SIZE] {
        unsafe { &mut MOVE_CMP_MAP }
    }

    fn state_changed(&self) -> bool {
        unsafe { MOVE_STATE_CHANGED }
    }
}

mod tests {
    use std::borrow::Borrow;
    use std::cell::RefCell;
    use std::rc::Rc;
    use move_vm_types::loaded_data::runtime_types::CachedStructIndex;
    use move_vm_types::loaded_data::runtime_types::Type::Struct;
    use super::*;
    use crate::r#move::input::{CloneableValue};
    use crate::state::FuzzState;

    use move_vm_types::values::{ContainerRef, Reference, ReferenceImpl, Value, ValueImpl};

    fn _run(
        bytecode: &str,
        args: Vec<CloneableValue>,
        func: &str,
    ) -> ExecutionResult<ModuleId, AccountAddress, MoveVMState, MoveOutput> {
        let module_bytecode = hex::decode(bytecode).unwrap();
        let module = CompiledModule::deserialize(&module_bytecode).unwrap();
        let module_idx = module.self_id();
        let mut mv = MoveVM::<
            MoveFunctionInput,
            FuzzState<MoveFunctionInput, MoveVMState, ModuleId, AccountAddress, MoveOutput>,
        >::new();
        let _loc = mv
            .deploy(
                module,
                None,
                AccountAddress::new([0; 32]),
                &mut FuzzState::new(0),
            )
            .unwrap();

        assert_eq!(mv.functions.len(), 1);

        let input = MoveFunctionInput {
            // take the first module
            module: mv.loader.module_cache.read().modules.id_map.iter().next().unwrap().0.clone(),
            function: Identifier::new(func).unwrap(),
            function_info: Default::default(),
            args,
            ty_args: vec![],
            caller: AccountAddress::new([1; 32]),
            vm_state: StagedVMState {
                state: MoveVMState {
                    resources: Default::default(),
                    _gv_slot: Default::default(),
                    value_to_drop: Default::default(),
                    useful_value: Default::default(),
                    ref_in_use: vec![],
                },
                stage: vec![],
                initialized: false,
                trace: Default::default(),
            },
            vm_state_idx: 0,
            _deps: Default::default(),
        };
        let mut res= ExecutionResult::empty_result();
        res = mv.execute(&input.clone(), &mut FuzzState::new(0));
        return res;
    }

    #[test]
    fn test_move_vm_simple() {
        // module 0x3::TestMod {
        //         public fun test1(data: u64) : u64 {
        //         data * 2
        //     }
        // }

        let module_hex = "a11ceb0b0500000006010002030205050703070a0e0818200c38130000000100000001030007546573744d6f6405746573743100000000000000000000000000000000000000000000000000000000000000030001000001040b00060200000000000000180200";
        _run(module_hex,
             vec![CloneableValue::from(Value::u64(20))],
                "test1",
        );
    }

    #[test]
    fn test_dropping() {
        // module 0x3::TestMod {
        //     resource struct TestStruct {
        //         data: u64
        //     }
        //     public fun test1(data: u64) : TestStruct {
        //         TestStruct { data };
        //     }
        // }

        let module_hex = "a11ceb0b0500000008010002020204030605050b0607111e082f200a4f050c540b000000010200000200010001030108000007546573744d6f640a546573745374727563740574657374310464617461000000000000000000000000000000000000000000000000000000000000000300020103030001000002030b0012000200";
        _run(module_hex,
             vec![CloneableValue::from(Value::u64(20))],
             "test1",
        );
    }

    #[test]
    fn test_args() {
        let module_hex = "a11ceb0b060000000901000202020403060a05100a071a290843200a63070c6a270d91010200020000020000040001000003020300000108000106080001030b50726f66696c65496e666f046e616d650770726f66696c650574657374310574657374320375726c0000000000000000000000000000000000000000000000000000000000000000000202010305030001000000040601000000000000000602000000000000001200020101000000040b0010001402000000";
        let res = _run(module_hex,
             vec![],
             "test2",
        );

        println!("{:?}", res);
        let (ty, struct_obj) = res.output.vars[0].clone();
        assert_eq!(ty, Struct(CachedStructIndex {
            0: 0,
        }));

        if let ValueImpl::Container(borrowed) = struct_obj.0.borrow() {
            println!("borrowed: {:?} from {:?}", borrowed, struct_obj);
            let reference = Value(ValueImpl::ContainerRef(ContainerRef::Local(
                borrowed.copy_by_ref(),
            )));

            println!("reference: {:?} from {:?}", reference, struct_obj);


            let res2 = _run(module_hex,
                            vec![CloneableValue::from(reference)],
                            "test1",
            );

            println!("{:?}", res2);
        } else {
            unreachable!()
        }


    }

    #[test]
    fn test_use_stdlib() {

        let module_hex = "a11ceb0b060000000801000202020403060a0510080718290841200a61070c68240002000002000004000100000301020001070200010301020b50726f66696c65496e666f046e616d650770726f66696c650574657374310574657374320375726c00000000000000000000000000000000000000000000000000000000000000000002020103050300010000010431030b00150201010000030631020c000d001100060c000000000000000200";
        let res = _run(module_hex,
                       vec![
                           CloneableValue::from(Value(ValueImpl::IndexedRef(
                                 values::IndexedRef {
                                     idx: 0,
                                     container_ref: ContainerRef::Local(
                                         values::Container::Locals(
                                             Rc::new(RefCell::new(vec![ValueImpl::U8(2)]))
                                         )
                                     ),
                                 }
                           ))),
                       ],
                       "test2",
        );
    }
}
