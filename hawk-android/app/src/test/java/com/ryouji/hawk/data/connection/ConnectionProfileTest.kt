package com.ryouji.hawk.data.connection

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class ConnectionProfileTest {
    @Test
    fun `lan address gets http scheme`() {
        val profile = ConnectionProfile(kind = ConnectionKind.Lan, url = "192.168.1.5:27372")
        assertEquals("http://192.168.1.5:27372/", profile.baseUrl().toString())
    }

    @Test
    fun `remote address gets https scheme`() {
        val profile = ConnectionProfile(kind = ConnectionKind.Remote, url = "hawk.example.com/sub")
        assertEquals("https://hawk.example.com/sub/", profile.baseUrl().toString())
    }

    @Test
    fun `strips query fragment and trailing slashes`() {
        val profile = ConnectionProfile(
            kind = ConnectionKind.Lan,
            url = "http://192.168.1.5:27372/?token=tk_1#x",
        )
        assertEquals("http://192.168.1.5:27372/", profile.baseUrl().toString())
    }

    @Test
    fun `keeps deployment sub path`() {
        val profile = ConnectionProfile(
            kind = ConnectionKind.Remote,
            url = "https://example.com/hawk",
        )
        assertEquals("https://example.com/hawk/", profile.baseUrl().toString())
    }

    @Test
    fun `rejects empty address`() {
        val error = assertThrows(ConnectionException::class.java) {
            ConnectionProfile(kind = ConnectionKind.Lan, url = "  ").baseUrl()
        }
        assertEquals("请填写服务地址", error.message)
    }

    @Test
    fun `display name falls back to host`() {
        val profile = ConnectionProfile(kind = ConnectionKind.Lan, url = "192.168.1.5:27372")
        assertEquals("192.168.1.5", profile.displayName)
    }
}
