// Tiny patch-target. The integration test in rec/Earthfile rewrites
// the return value with `sed` between two wild builds and emits a v3
// patch describing the diff. Keep the literal `-> u32 { 1 }` shape
// stable so the sed pattern keeps matching.

#[inline(never)]
#[no_mangle]
pub fn compute() -> u32 { 1 }
