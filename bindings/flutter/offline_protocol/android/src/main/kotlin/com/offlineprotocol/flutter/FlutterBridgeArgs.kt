package com.offlineprotocol.flutter

internal object FlutterBridgeArgs {
    fun requireString(map: Map<*, *>, key: String): String =
        (map[key] as? String)?.takeIf { it.isNotEmpty() }
            ?: throw IllegalArgumentException("Missing or empty '$key'")

    fun optionalString(map: Map<*, *>, key: String): String? =
        (map[key] as? String)?.takeIf { it.isNotEmpty() }

    fun requireBoolean(map: Map<*, *>, key: String): Boolean =
        when (val v = map[key]) {
            is Boolean -> v
            else -> throw IllegalArgumentException("Missing boolean '$key'")
        }

    fun requireInt(map: Map<*, *>, key: String): Int =
        when (val v = map[key]) {
            is Int -> v
            is Number -> v.toInt()
            else -> throw IllegalArgumentException("Missing int '$key'")
        }

    fun optionalInt(map: Map<*, *>, key: String): Int? =
        when (val v = map[key]) {
            null -> null
            is Int -> v
            is Number -> v.toInt()
            else -> throw IllegalArgumentException("Invalid int '$key'")
        }

    fun requireDouble(map: Map<*, *>, key: String): Double =
        when (val v = map[key]) {
            is Double -> v
            is Number -> v.toDouble()
            else -> throw IllegalArgumentException("Missing number '$key'")
        }

    fun requireStringList(map: Map<*, *>, key: String): List<String> {
        val raw = map[key] ?: return emptyList()
        if (raw is List<*>) {
            return raw.mapNotNull { it as? String }
        }
        throw IllegalArgumentException("'$key' must be a list of strings")
    }

    fun requireUByteList(map: Map<*, *>, key: String): List<UByte> {
        val raw = map[key] ?: throw IllegalArgumentException("Missing '$key'")
        // A Dart Uint8List arrives as a ByteArray (the standard codec).
        if (raw is ByteArray) return raw.map { it.toUByte() }
        if (raw is List<*>) {
            return raw.map {
                when (it) {
                    is Int -> it.toUByte()
                    is Number -> it.toInt().toUByte()
                    else -> throw IllegalArgumentException("Invalid byte in '$key'")
                }
            }
        }
        throw IllegalArgumentException("'$key' must be a list of ints")
    }

    fun mapOrWrap(arguments: Any?): Map<*, *> = when (arguments) {
        is Map<*, *> -> arguments
        else -> throw IllegalArgumentException("Expected a map argument")
    }
}
