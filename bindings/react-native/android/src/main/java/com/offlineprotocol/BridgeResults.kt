package com.offlineprotocol

import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.ReadableArray
import com.facebook.react.bridge.ReadableMap
import com.facebook.react.bridge.ReadableType
import com.facebook.react.bridge.WritableArray
import com.facebook.react.bridge.WritableMap
import org.json.JSONArray
import org.json.JSONObject

/**
 * Converts [ProtocolEngineHostBridge] return values to React Native promise payloads.
 *
 * The shape each method resolves is the one it resolved before the shared bridge
 * existed, because `src/index.ts` was written against it. Two rules hold that:
 *
 * - A [String] is resolved as the string, whatever it contains. `getTopology` and
 *   `getMessageStats` return JSON that `src/index.ts` passes to `JSON.parse`, and
 *   `JSON.parse` on an object throws. Parsing here by content would also make the
 *   type of a free-text result depend on what the user typed.
 * - [parseJsonResult] is the one way to turn a JSON string into an object, and the
 *   generated adapter calls it only for methods the manifest marks
 *   `parseJsonResult` (`getDorsConfig`, which React Native has always resolved as
 *   an object and the shared bridge returns as JSON for Flutter's codec).
 *
 * Numbers keep their fraction: `getDorsConfig` carries ratios such as
 * `switchHysteresis`, and narrowing a JSON double to a long reports 0.3 as 0.
 */
internal object BridgeResults {
    /**
     * A JavaScript array as a list the shared bridge reads, each element kept as
     * its own type: a string stays a string (`sendReadReceipt`'s message ids), a
     * whole number becomes an [Int] (every byte array), any other number a
     * [Double]. Reading every element with `getInt` throws on the first string,
     * which is how `sendReadReceipt` failed on every call in 0.31.0.
     */
    fun readableArrayToList(array: ReadableArray?): List<Any?>? {
        if (array == null) return null
        return List(array.size()) { i ->
            when (array.getType(i)) {
                ReadableType.Null -> null
                ReadableType.Boolean -> array.getBoolean(i)
                ReadableType.Number -> {
                    val number = array.getDouble(i)
                    if (number % 1.0 == 0.0 && number >= Int.MIN_VALUE && number <= Int.MAX_VALUE) {
                        number.toInt()
                    } else {
                        number
                    }
                }
                ReadableType.String -> array.getString(i)
                ReadableType.Map -> readableMapToArgumentMap(array.getMap(i))
                ReadableType.Array -> readableArrayToList(array.getArray(i))
            }
        }
    }

    fun readableMapToArgumentMap(map: ReadableMap?): Map<String, Any?> {
        if (map == null) return emptyMap()
        return BridgeArgumentMaps.readableMapToMap(map)
    }

    fun toReactNative(value: Any?): Any? = when (value) {
        null -> null
        is Boolean, is String, is Int, is Double -> value
        is Float -> value.toDouble()
        is Long -> if (value in Int.MIN_VALUE..Int.MAX_VALUE) value.toInt() else value.toDouble()
        is Short -> value.toInt()
        is Byte -> value.toInt()
        is UInt -> toReactNative(value.toLong())
        is ULong -> value.toDouble()
        is UShort -> value.toInt()
        is UByte -> value.toInt()
        is Map<*, *> -> mapToWritable(value.entries.associate { it.key.toString() to it.value })
        is List<*> -> listToWritable(value)
        is JSONObject -> jsonObjectToWritable(value)
        is JSONArray -> jsonArrayToWritable(value)
        else -> value.toString()
    }

    /**
     * Resolves a JSON object or array string as the object or array, and any other
     * value as [toReactNative] does. A string that is not valid JSON is resolved as
     * the string, so a malformed result reaches JavaScript rather than a crash.
     */
    fun parseJsonResult(value: Any?): Any? {
        if (value is String) {
            val trimmed = value.trimStart()
            if (trimmed.startsWith("{")) {
                return runCatching { jsonObjectToWritable(JSONObject(value)) }.getOrElse { value }
            }
            if (trimmed.startsWith("[")) {
                return runCatching { jsonArrayToWritable(JSONArray(value)) }.getOrElse { value }
            }
        }
        return toReactNative(value)
    }

    private fun mapToWritable(map: Map<String, Any?>): WritableMap {
        val out = Arguments.createMap()
        map.forEach { (key, raw) -> putValue(out, key, toReactNative(raw)) }
        return out
    }

    private fun listToWritable(list: List<*>): WritableArray {
        val out = Arguments.createArray()
        list.forEach { item -> pushValue(out, toReactNative(item)) }
        return out
    }

    private fun jsonObjectToWritable(json: JSONObject): WritableMap {
        val out = Arguments.createMap()
        json.keys().forEach { key ->
            val v = json.opt(key)
            putValue(out, key, if (v == JSONObject.NULL) null else toReactNative(v))
        }
        return out
    }

    private fun jsonArrayToWritable(json: JSONArray): WritableArray {
        val out = Arguments.createArray()
        for (i in 0 until json.length()) {
            val v = json.opt(i)
            pushValue(out, if (v == JSONObject.NULL) null else toReactNative(v))
        }
        return out
    }

    /** [value] is already normalised by [toReactNative]. */
    private fun putValue(map: WritableMap, key: String, value: Any?) {
        when (value) {
            null -> map.putNull(key)
            is Boolean -> map.putBoolean(key, value)
            is String -> map.putString(key, value)
            is Int -> map.putInt(key, value)
            is Double -> map.putDouble(key, value)
            is WritableMap -> map.putMap(key, value)
            is WritableArray -> map.putArray(key, value)
            else -> map.putString(key, value.toString())
        }
    }

    /** [value] is already normalised by [toReactNative]. */
    private fun pushValue(array: WritableArray, value: Any?) {
        when (value) {
            null -> array.pushNull()
            is Boolean -> array.pushBoolean(value)
            is String -> array.pushString(value)
            is Int -> array.pushInt(value)
            is Double -> array.pushDouble(value)
            is WritableMap -> array.pushMap(value)
            is WritableArray -> array.pushArray(value)
            else -> array.pushString(value.toString())
        }
    }
}
