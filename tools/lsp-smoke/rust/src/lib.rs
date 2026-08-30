pub fn bli_kvm_double_value(value: i32) -> i32 {
    value * 2
}

pub fn bli_kvm_rust_probe() -> i32 {
    bli_kvm_double_value(21)
}
