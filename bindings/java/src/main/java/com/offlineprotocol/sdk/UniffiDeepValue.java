package com.offlineprotocol.sdk;


// Value equality for generated fields whose rendering holds a Java array at some depth:
// primitive arrays from `Vec<prim>`, `byte[]` from `bytes`, possibly under lists, maps, or null.
// Arrays compare and hash by identity, so `Objects.equals` is wrong for them anywhere it would
// reach one. Map keys never hold arrays (hashed positions render boxed), so key lookups here
// match by value.
final class UniffiDeepValue {
    private UniffiDeepValue() {}

    static boolean equals(java.lang.Object a, java.lang.Object b) {
        if (a == b) return true;
        if (a instanceof java.util.List<?> x && b instanceof java.util.List<?> y) {
            if (x.size() != y.size()) return false;
            java.util.Iterator<?> i = x.iterator();
            java.util.Iterator<?> j = y.iterator();
            while (i.hasNext()) {
                if (!equals(i.next(), j.next())) return false;
            }
            return true;
        }
        if (a instanceof java.util.Map<?, ?> x && b instanceof java.util.Map<?, ?> y) {
            if (x.size() != y.size()) return false;
            for (java.util.Map.Entry<?, ?> e : x.entrySet()) {
                if (!y.containsKey(e.getKey()) || !equals(e.getValue(), y.get(e.getKey()))) {
                    return false;
                }
            }
            return true;
        }
        // Covers every primitive array type, so no per-type Arrays.equals arms are needed.
        return java.util.Objects.deepEquals(a, b);
    }

    // Per-type arms rather than an Arrays.deepHashCode wrapper: equals has an allocation-free
    // JDK entry point in Objects.deepEquals, hashCode does not.
    static int hashCode(java.lang.Object o) {
        if (o == null) return 0;
        if (o instanceof byte[] x) return java.util.Arrays.hashCode(x);
        if (o instanceof short[] x) return java.util.Arrays.hashCode(x);
        if (o instanceof int[] x) return java.util.Arrays.hashCode(x);
        if (o instanceof long[] x) return java.util.Arrays.hashCode(x);
        if (o instanceof float[] x) return java.util.Arrays.hashCode(x);
        if (o instanceof double[] x) return java.util.Arrays.hashCode(x);
        if (o instanceof boolean[] x) return java.util.Arrays.hashCode(x);
        if (o instanceof java.util.List<?> x) {
            // The List.hashCode contract, with array-aware element hashes.
            int result = 1;
            for (java.lang.Object e : x) {
                result = 31 * result + hashCode(e);
            }
            return result;
        }
        if (o instanceof java.util.Map<?, ?> x) {
            // The Map.hashCode contract, with array-aware value hashes.
            int result = 0;
            for (java.util.Map.Entry<?, ?> e : x.entrySet()) {
                result += java.util.Objects.hashCode(e.getKey()) ^ hashCode(e.getValue());
            }
            return result;
        }
        return o.hashCode();
    }
}
