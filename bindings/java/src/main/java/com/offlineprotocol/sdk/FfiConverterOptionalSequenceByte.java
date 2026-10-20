package com.offlineprotocol.sdk;

public enum FfiConverterOptionalSequenceByte implements FfiConverterRustBuffer<java.util.List<java.lang.Byte>> {
  INSTANCE;

  @Override
  public java.util.List<java.lang.Byte> read(java.nio.ByteBuffer buf) {
    if (buf.get() == (byte)0) {
      return null;
    }
    return FfiConverterSequenceByte.INSTANCE.read(buf);
  }

  @Override
  public long allocationSize(java.util.List<java.lang.Byte> value) {
    if (value == null) {
      return 1L;
    } else {
      return 1L + FfiConverterSequenceByte.INSTANCE.allocationSize(value);
    }
  }

  @Override
  public void write(java.util.List<java.lang.Byte> value, java.nio.ByteBuffer buf) {
    if (value == null) {
      buf.put((byte)0);
    } else {
      buf.put((byte)1);
      FfiConverterSequenceByte.INSTANCE.write(value, buf);
    }
  }
}
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */
