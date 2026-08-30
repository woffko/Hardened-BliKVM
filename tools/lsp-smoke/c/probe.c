int bli_kvm_double_value(int value) {
    return value * 2;
}

int bli_kvm_c_probe(void) {
    return bli_kvm_double_value(21);
}
