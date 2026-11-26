(module
  (memory (export "memory") 1)

  ;; Starling ABI v1: the host allocates a request buffer through this export.
  ;; This example uses a simple bump allocator and ignores the request contents.
  (global $next (mut i32) (i32.const 1024))
  (func (export "starling_abi_version") (result i32)
    i32.const 1)
  (func (export "starling_alloc") (param $size i32) (result i32)
    (local $ptr i32)
    global.get $next
    local.set $ptr
    global.get $next
    local.get $size
    i32.add
    global.set $next
    local.get $ptr)

  ;; Packed i64 response: (pointer << 32) | length.
  (func (export "starling_handle") (param $ptr i32) (param $len i32) (result i64)
    i64.const 17592186044495)

  (data (i32.const 4096) "\53\54\57\4c\52\53\30\31\01\00\c8\00\02\00\0c\00\43\6f\6e\74\65\6e\74\2d\54\79\70\65\0a\00\00\00\74\65\78\74\2f\70\6c\61\69\6e\06\00\58\2d\57\61\73\6d\05\00\00\00\68\65\6c\6c\6f\10\00\00\00\68\65\6c\6c\6f\20\66\72\6f\6d\20\77\61\73\6d\0a")
)
