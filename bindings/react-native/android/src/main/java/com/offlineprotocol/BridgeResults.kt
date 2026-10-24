package com.offlineprotocol

import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.ReadableArray
import com.facebook.react.bridge.ReadableMap
import com.facebook.react.bridge.WritableArray
import com.facebook.react.bridge.WritableMap
import org.json.JSONArray
import org.json.JSONObject

/** Converts [ProtocolEngineHostBridge] return values to React Native promise payloads. */
internal object BridgeResults {
    fun readableArrayToIntList(array: ReadableArray?): List<Int>? {
        if (array == null) return null
        return List(array.size()) { i -> array.getInt(i) }
    }

    fun readableMapToArgumentMap(map: ReadableMap?): Map<String, Any?> {
        if (map == null) return emptyMap()
        return BridgeArgumentMaps.readableMapToMap(map)
    }

    fun toReactNative(value: Any?): Any? = when (value) {
        null -> null
        is Boolean, is String, is Int, is Double, is Float -> value
        is Long -> if (value in Int.MIN_VALUE..Int.MAX_VALUE) value.toInt() else value.toDouble()
        is Map<*, *> -> mapToWritable(value.entries.associate { it.key.toString() to it.value })
        is List<*> -> listToWritable(value)
        is JSONObject -> jsonObjectToWritable(value)
        is JSONArray -> jsonArrayToWritable(value)
        else -> value.toString()
    }

    /**
     * Bridge methods that return JSON objects as [String] (Flutter channel shape).
     * Parse and emit a [WritableMap] for React Native.
     */
    fun toReactNativeJsonStringOrPassThrough(value: Any?): Any? {
        if (value is String && value.startsWith("{")) {
            return runCatching { jsonObjectToWritable(JSONObject(value)) }
                .getOrElse { value }
        }
        if (value is String && value.startsWith("[")) {
            return runCatching { jsonArrayToWritable(JSONArray(value)) }
                .getOrElse { value }
        }
        return toReactNative(value)
    }

    private fun mapToWritable(map: Map<String, Any?>): WritableMap {
        val out = Arguments.createMap()
        map.forEach { (key, raw) ->
            when (val v = raw) {
                null -> out.putNull(key)
                is Boolean -> out.putBoolean(key, v)
                is String -> out.putString(key, v)
                is Int -> out.putInt(key, v)
                is Double -> out.putDouble(key, v)
                is Float -> out.putDouble(key, v.toDouble())
                is Long -> putNumber(out, key, v)
                is Map<*, *> -> out.putMap(key, mapToWritable(v.entries.associate { it.key.toString() to it.value }))
                is List<*> -> out.putArray(key, listToWritable(v))
                else -> out.putString(key, v.toString())
            }
        }
        return out
    }

    private fun listToWritable(list: List<*>): WritableArray {
        val out = Arguments.createArray()
        list.forEach { item ->
            when (item) {
                null -> out.pushNull()
                is Boolean -> out.pushBoolean(item)
                is String -> out.pushString(item)
                is Int -> out.pushInt(item)
                is Double -> out.pushDouble(item)
                is Float -> out.pushDouble(item.toDouble())
                is Long -> pushNumber(out, item)
                is Map<*, *> -> out.pushMap(mapToWritable(item.entries.associate { it.key.toString() to it.value }))
                is List<*> -> out.pushArray(listToWritable(item))
                else -> out.pushString(item.toString())
            }
        }
        return out
    }

    private fun jsonObjectToWritable(json: JSONObject): WritableMap {
        val out = Arguments.createMap()
        json.keys().forEach { key ->
            when (val v = json.opt(key)) {
                JSONObject.NULL, null -> out.putNull(key)
                is Boolean -> out.putBoolean(key, v)
                is String -> out.putString(key, v)
                is Int -> out.putInt(key, v)
                is Long -> putNumber(out, key, v)
                is Double -> putNumber(out, key, v.toLong())
                is JSONObject -> out.putMap(key, jsonObjectToWritable(v))
                is JSONArray -> out.putArray(key, jsonArrayToWritable(v))
                else -> out.putString(key, v.toString())
            }
        }
        return out
    }

    private fun jsonArrayToWritable(json: JSONArray): WritableArray {
        val out = Arguments.createArray()
        for (i in 0 until json.length()) {
            when (val v = json.opt(i)) {
                JSONObject.NULL, null -> out.pushNull()
                is Boolean -> out.pushBoolean(v)
                is String -> out.pushString(v)
                is Int -> out.pushInt(v)
                is Long -> pushNumber(out, v)
                is Double -> pushNumber(out, v.toLong())
                is JSONObject -> out.pushMap(jsonObjectToWritable(v))
                is JSONArray -> out.pushArray(jsonArrayToWritable(v))
                else -> out.pushString(v.toString())
            }
        }
        return out
    }

    private fun putNumber(map: WritableMap, key: String, n: Long) {
        if (n in Int.MIN_VALUE..Int.MAX_VALUE && n.toDouble() == n.toInt().toDouble()) {
            map.putInt(key, n.toInt())
        } else {
            map.putDouble(key, n.toDouble())
        }
    }

    private fun putNumber(map: WritableMap, key: String, n: Double) {
        if (n.isFinite() && n == n.toLong().toDouble() && n.toLong() in Int.MIN_VALUE..Int.MAX_VALUE) {
            map.putInt(key, n.toInt())
        } else {
            map.putDouble(key, n)
        }
    }

    private fun pushNumber(array: WritableArray, n: Long) {
        if (n in Int.MIN_VALUE..Int.MAX_VALUE) {
            array.pushInt(n.toInt())
        } else {
            array.pushDouble(n.toDouble())
        }
    }
}
