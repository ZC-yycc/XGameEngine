use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    // libtest 在不同线程跑用例，计数只看量测线程，避免记录其他用例的分配。
    static COUNTER: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}

pub(crate) fn snapshot() -> Option<(u64, u64)> {
    COUNTER.try_with(Cell::get).ok()
}

pub(crate) fn reset_counter() {
    let _ = COUNTER.try_with(|counter| counter.set((0, 0)));
}

struct CountingAllocator;

// SAFETY: 把布局和指针原样交给系统分配器，只记录分配成功后的计数。
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        record(pointer, layout.size());
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let pointer = unsafe { System.realloc(pointer, layout, new_size) };
        record(pointer, new_size);
        pointer
    }
}

fn record(pointer: *mut u8, size: usize) {
    if !pointer.is_null() {
        let _ = COUNTER.try_with(|counter| {
            let (calls, bytes) = counter.get();
            // realloc 计整块新尺寸。字节数是申请量，不代表净增长或峰值内存。
            counter.set((calls + 1, bytes + size as u64));
        });
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
