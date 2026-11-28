package com.ryouji.hawk.data.connection

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class ConnectionProfileTest {
    @Test
    fun `lan address gets http scheme`() {
        val profile = ConnectionProfile(url = "192.168.1.5:27375")
        assertEquals("http://192.168.1.5:27375/", profile.baseUrl().toString())
    }

    @Test
    fun `keeps explicit https address and sub path`() {
        val profile = ConnectionProfile(url = "https://example.com/hawk")
        assertEquals("https://example.com/hawk/", profile.baseUrl().toString())
    }

    @Test
    fun `strips query fragment and trailing slashes`() {
        val profile = ConnectionProfile(
            url = "http://192.168.1.5:27375/?token=tk_1#x",
        )
        assertEquals("http://192.168.1.5:27375/", profile.baseUrl().toString())
    }

    @Test
    fun `rejects unsupported scheme`() {
        val error = assertThrows(ConnectionException::class.java) {
            ConnectionProfile(url = "ftp://192.168.1.5:27375").baseUrl()
        }
        assertEquals("服务地址无效，请填写形如 192.168.1.5:27375 的地址", error.message)
    }

    @Test
    fun `rejects empty address`() {
        val error = assertThrows(ConnectionException::class.java) {
            ConnectionProfile(url = "  ").baseUrl()
        }
        assertEquals("请填写服务地址", error.message)
    }

    @Test
    fun `display name falls back to host`() {
        val profile = ConnectionProfile(url = "192.168.1.5:27375")
        assertEquals("192.168.1.5", profile.displayName)
    }

    @Test
    fun `candidate urls deduplicate and keep order`() {
        val profile = ConnectionProfile(
            url = "192.168.1.5:27375",
            urls = listOf("192.168.1.5:27375", "192.168.1.6:27375"),
        )
        assertEquals(
            listOf("http://192.168.1.5:27375/", "http://192.168.1.6:27375/"),
            profile.candidateBaseUrls().map { it.toString() },
        )
    }
}
