# frozen_string_literal: true

# fmt 11.0.2 (hard-pinned by RCT-Folly -> React Native 0.82) does not compile
# under Xcode 26+ / clang 21: clang defines __cpp_consteval, so fmt's base.h
# selects FMT_USE_CONSTEVAL=1 and its consteval format-string check fails with
# "call to consteval function ... is not a constant expression". base.h sets
# FMT_USE_CONSTEVAL via a raw #if/#elif chain with NO #ifndef guard, so a
# -DFMT_USE_CONSTEVAL=0 compile flag is silently overridden — the value must be
# forced in the header. Idempotent; re-applied on every `pod install`.
def apply_fmt_consteval_patch_for_xcode26(installer)
  fmt_base = File.join(installer.sandbox.root, 'fmt', 'include', 'fmt', 'base.h')
  return unless File.exist?(fmt_base)

  original = File.read(fmt_base)
  patched = original.gsub('#  define FMT_USE_CONSTEVAL 1',
                          '#  define FMT_USE_CONSTEVAL 0')
  return if patched == original

  File.write(fmt_base, patched)
  Pod::UI.puts 'Patched fmt/base.h: forced FMT_USE_CONSTEVAL=0 (Xcode 26+ / clang 21)'.yellow
end
