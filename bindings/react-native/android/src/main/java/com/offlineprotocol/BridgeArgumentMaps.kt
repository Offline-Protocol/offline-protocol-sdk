package com.offlineprotocol

import com.facebook.react.bridge.ReadableArray
import com.facebook.react.bridge.ReadableMap
import com.facebook.react.bridge.ReadableType
import org.json.JSONArray
import org.json.JSONObject

/** Converts React Native [ReadableMap] args to maps the shared engine host accepts. */
internal object BridgeArgumentMaps {
    fun readableMapToMap(map: ReadableMap?): Map<String, Any?> {
        if (map == null) return emptyMap()
        val out = linkedMapOf<String, Any?>()
        val iterator = map.entryIterator
        while (iterator.hasNext()) {
            val entry = iterator.next()
            out[entry.key] = readableToAny(entry.value)
        }
        return out
    }

    fun readableMapToJsonObject(map: ReadableMap?): JSONObject? {
        if (map == null) return null
        val json = JSONObject()
        val iterator = map.entryIterator
        while (iterator.hasNext()) {
            val entry = iterator.next()
            json.put(entry.key, readableToJson(entry.value))
        }
        return json
    }

    private fun readableToAny(value: Any?): Any? = when (value) {
        null -> null
        is ReadableMap -> readableMapToMap(value)
        is ReadableArray -> readableArrayToList(value)
        else -> value
    }

    private fun readableToJson(value: Any?): Any? = when (value) {
        null -> JSONObject.NULL
        is Boolean -> value
        is Int -> value
        is Long -> value
        is Double -> value
        is String -> value
        is ReadableMap -> readableMapToJsonObject(value) ?: JSONObject()
        is ReadableArray -> readableArrayToJsonArray(value)
        else -> value.toString()
    }

    private fun readableArrayToList(array: ReadableArray): List<Any?> {
        val list = ArrayList<Any?>(array.size())
        for (i in 0 until array.size()) {
            list.add(
                when (array.getType(i)) {
                    ReadableType.Null -> null
                    ReadableType.Boolean -> array.getBoolean(i)
                    ReadableType.Number -> array.getDouble(i)
                    ReadableType.String -> array.getString(i)
                    ReadableType.Map -> readableMapToMap(array.getMap(i))
                    ReadableType.Array -> readableArrayToList(array.getArray(i)!!)
                },
            )
        }
        return list
    }

    private fun readableArrayToJsonArray(array: ReadableArray): JSONArray {
        val json = JSONArray()
        for (i in 0 until array.size()) {
            json.put(
                when (array.getType(i)) {
                    ReadableType.Null -> JSONObject.NULL
                    ReadableType.Boolean -> array.getBoolean(i)
                    ReadableType.Number -> array.getDouble(i)
                    ReadableType.String -> array.getString(i)
                    ReadableType.Map -> readableMapToJsonObject(array.getMap(i))
                    ReadableType.Array -> readableArrayToJsonArray(array.getArray(i)!!)
                },
            )
        }
        return json
    }
}
