# 0025. Native packages are assembled from the bridge sources in place

**Status:** Accepted

## Context

A native iOS or Android application had no package to install. Each release
attached a zip holding the Rust library and the generated binding, and the
integration guides said to build from source. The code that turns the binding
into a working SDK, the transport managers and the storage providers, shipped
only inside the React Native package.

Three facts shaped what a native package could be.

**Almost none of the bridge needs React.** Of the hand-written Kotlin, three
files import it: the module, its package registration, and the headless wake
service. Of the Swift, one does, with its Objective-C shim. Every transport
manager, both storage providers and every policy class compile without it.
That was measured, not read: the Android sources with those three files left
out compile and pass their whole unit suite, and the Swift sources build,
link and run as a package on a simulator.

**The sources are read where they are.** About fifty Rust guards open a Swift
or Kotlin bridge source by its path under `bindings/react-native` and assert
something about its text, because nothing else can: no compiler sees a
constant that exists in three languages
([C5](../bridges/README.md#c5-hand-mirrored-constants-must-be-pinned-in-every-language)).
Most of them panic when the file is missing. Four skip instead, on purpose,
so that a crate vendored outside this repository still passes its tests, and
three list a directory and take whatever is in it. A source that moved would
break the first kind and quietly stop being checked by the other two.

**Swift Package Manager resolves a manifest at a tag.** A binary target names
the checksum of an archive, and the archive is built by the release, which
runs after the tag exists. The manifest at the tag cannot hold a checksum
that does not exist yet.

## Decision

1. **A bridge source has one home.** It stays under
   `bindings/react-native/ios` or `bindings/react-native/android`. A native
   package takes it from there and nothing moves or is copied into the
   repository.
2. **The packages are assembled.** The Swift package is a directory written
   by `scripts/assemble-swift-package.sh`. The Android library is a Gradle
   build under `bindings/kotlin` whose source sets point at the directories
   above. Both builds run in CI on every pull request.
3. **What needs React is a short, named list of what to leave out.** Not a
   list of what to take: a new bridge source is part of both packages without
   being registered anywhere, where the React Native package already takes
   sources without being told. On iOS that is the top level of `ios/`, and
   the two directories the podspec takes whole.
4. **The package build is the check on that list.** A shared source that
   starts to need React fails the package build, in the pull request that
   made it so.
5. **A value the React Native package declares, and that would drift
   without anything failing, is read and not written again.** The iOS
   deployment target comes from the podspec. The Android SDK levels and the
   version of every dependency come from the module's build file, the
   compiler and the Android plugin from its test harness, and the manifest
   from the module's manifest, rewritten at build time without the entries
   React needs. Each is read from a line that declares it, which has to be
   there exactly once.
6. **Code that exists only for a native application lives in
   `bindings/swift` and `bindings/kotlin`.**
7. **The Swift package ships through a distribution repository that is
   generated output.** The release workflow writes the assembled tree there
   and tags it, after the archive it names is published. Nobody edits that
   repository.
8. **Neither package carries a version of its own.** Both take it from the
   tag, so there is nothing for the version gate to compare.

## Consequences

- The React Native package is unaffected. It compiles the same files from the
  same place.
- Three iOS suites that ran nowhere now run: the mesh controller, the BLE
  discovery bootstrap policy and the error mapping are excluded from the
  SwiftPM test harness, which cannot compile the sources they cover. Two
  tests in the first were failing, unseen. Their Kotlin twins had failed the
  same way and were repaired when Android got a CI job.
- The Android library is built by the toolchain the module's tests run on,
  which is older than the current one. That is deliberate. Kotlin refuses
  metadata more than one version ahead of the compiler reading it, so a
  library compiled by a newer Kotlin than its consumers use cannot be used
  by them at all, and one compiled by a newer Kotlin than its tests is not
  the code that was tested. It rules out publishing plugins that need a
  newer Gradle. The library is published by Gradle's own.
- A release has two more channels that cannot take a version back. A Swift
  tag must never move, because Swift Package Manager records the revision
  behind each version and refuses one that changed, and a version on a Maven
  repository is permanent. A bad release is answered by the next one.
- A release AAR carries the four legal files the Swift package carries
  (`LICENSE`, `LICENSE-COMMERCIAL.md`, `THIRD-PARTY-NOTICES.md`,
  `EXPORT.md`). Both packages hold the Rust library, and the notices are
  what its licenses ask to travel with it.
- On iOS the storage providers are internal, so the Swift package needs a
  public entry point before an application can use the built-in stores. On
  Android they can be constructed, and everything that was public in the
  module is public in the library. Nobody chose either surface as an API:
  it is what the React Native module happened to need. It has to be chosen
  before the first release.

## Alternatives considered

**Move the shared sources to a neutral directory.** Cleaner to look at. It
breaks every guard that panics and blinds the ones that skip, in one change
that has to repoint all of them correctly, for no difference in what ships.

**Copy the sources into the packages.** Two copies of a transport manager
diverge, and the guards would read only one of them.

**Extract the orchestration out of the React Native module first,** so that
the module becomes an adapter over a shared runtime. It is where this should
end up. It rewrites two files of more than five thousand lines each, which
about twenty guards pin and which are verified on devices by hand, so it is not
where to start.

**A manifest at the root of this repository.** One repository instead of two.
It needs the archive built and its checksum committed before the tag, which
turns the release around for every channel: today a release that fails its
version gate is fixed by deleting the tag and pushing it again, and that
stops being true the moment a tagged commit must already hold the output of
its own build.

## What would undo this

Moving a bridge source, in any change that does not repoint every guard that
reads it and prove each one still fails when it should. The four guards that
skip will not say so.

Writing down, in a package, a value the React Native package already
declares: a deployment target, an SDK level, a dependency version, a
permission. The copy is right on the day it is made.

Adding a list of sources to take. The assemble script and the Gradle build
leave files out by name and take the rest, so that the packages cannot fall
behind the directory. A list of what to take is a sixth registration point
([S2](../bridges/swift.md#s2-five-registration-points-per-new-swift-file)),
and the one a new file is missing from is found by a consumer.

Editing the distribution repository by hand. The next release overwrites it.
