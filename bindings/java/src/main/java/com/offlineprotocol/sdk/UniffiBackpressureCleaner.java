package com.offlineprotocol.sdk;


// A Cleaner backed by PhantomReference + ReferenceQueue with opportunistic inline draining.
//
// How it works:
// 1. Each registered object gets a PhantomReference pointing at a ReferenceQueue.
//    A doubly-linked list keeps strong references to the PhantomRefs so they aren't
//    GC'd before their referents.
// 2. A daemon thread blocks on queue.remove() and runs cleanup actions (steady state).
// 3. On every register() call, we do a small non-blocking drain of the queue on the
//    calling thread. queue.poll() is ~nanoseconds when empty, so the overhead is
//    negligible, but under sustained allocation pressure it provides continuous
//    backpressure that prevents the cleanup backlog from growing unboundedly.
// 4. Explicit close()/clean() is idempotent - manual clean and GC-triggered clean
//    race via a volatile CAS, so the action runs at most once.
// 5. clean() is idempotent via VarHandle CAS. It synchronizes on the list sentinel
//    to unlink itself, preventing dead entries from accumulating.
class UniffiBackpressureCleaner implements UniffiCleaner {
    private static final int DRAIN_BATCH_SIZE = 8;

    private final java.lang.ref.ReferenceQueue<java.lang.Object> queue = new java.lang.ref.ReferenceQueue<>();
    // Sentinel node for the doubly-linked list. All list mutations synchronize on this.
    private final CleanableRef head = new CleanableRef();

    UniffiBackpressureCleaner() {
        Thread t = new Thread(() -> {
            while (true) {
                try {
                    java.lang.ref.Reference<?> ref = queue.remove();
                    if (ref instanceof CleanableRef) {
                        ((CleanableRef) ref).clean();
                    }
                } catch (InterruptedException e) {
                    break;
                }
            }
        });
        t.setDaemon(true);
        t.setName("uniffi-cleaner");
        t.start();
    }

    @Override
    public UniffiCleaner.Cleanable register(java.lang.Object value, java.lang.Runnable cleanUpTask) {
        // Opportunistic drain: process a few pending refs on this thread.
        // queue.poll() is essentially free when the queue is empty.
        for (int i = 0; i < DRAIN_BATCH_SIZE; i++) {
            java.lang.ref.Reference<?> ref = queue.poll();
            if (ref == null) break;
            if (ref instanceof CleanableRef) {
                ((CleanableRef) ref).clean();
            }
        }
        return new CleanableRef(head, value, queue, cleanUpTask);
    }

    // A PhantomReference that doubles as a Cleanable, stored in a doubly-linked list to
    // keep it alive until cleanup. The cleanup action is stored in a volatile field with
    // VarHandle CAS for idempotent clean(). clean() synchronizes on the list sentinel to
    // unlink, keeping the list trimmed.
    private static class CleanableRef extends java.lang.ref.PhantomReference<java.lang.Object> implements UniffiCleaner.Cleanable {
        private static final java.lang.invoke.VarHandle ACTION;
        static {
            try {
                ACTION = java.lang.invoke.MethodHandles.lookup()
                    .findVarHandle(CleanableRef.class, "action", java.lang.Runnable.class);
            } catch (ReflectiveOperationException e) {
                throw new ExceptionInInitializerError(e);
            }
        }

        private volatile java.lang.Runnable action;
        private final CleanableRef list; // sentinel to synchronize on
        private CleanableRef prev;
        private CleanableRef next;

        // Sentinel constructor.
        CleanableRef() {
            super(null, null);
            this.list = this;
            this.prev = this;
            this.next = this;
        }

        // Normal constructor - inserts itself into the list.
        CleanableRef(CleanableRef list, java.lang.Object referent, java.lang.ref.ReferenceQueue<java.lang.Object> q, java.lang.Runnable action) {
            super(referent, q);
            this.action = action;
            this.list = list;
            synchronized (list) {
                this.prev = list;
                this.next = list.next;
                list.next.prev = this;
                list.next = this;
            }
        }

        @Override
        public void clean() {
            // Atomic swap ensures the action runs at most once, even if called
            // concurrently from close() and the background cleaner thread.
            java.lang.Runnable a = (java.lang.Runnable) ACTION.getAndSet(this, null);
            if (a != null) {
                synchronized (list) {
                    // Because of the swap above this these guards shouldn't be necessary, but including them defensively
                    if (next != null) next.prev = prev;
                    if (prev != null) prev.next = next;
                    prev = null;
                    next = null;
                }
                a.run();
            }
        }
    }
}