package com.ryouji.hawk.data.connection

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class PairingPayloadTest {
    @Test
    fun `decodes lan payload`() {
        val payload = PairingPayload.decode(
            """{"v":1,"kind":"lan","url":"http://192.168.1.5:27372","token":"tk_1"}"""
        )
        val profile = payload.toProfile()
        assertEquals(ConnectionKind.Lan, profile.kind)
        assertEquals("http://192.168.1.5:27372", profile.url)
        assertEquals("tk_1", profile.token)
    }

    @Test
    fun `decodes remote payload`() {
        val payload = PairingPayload.decode(
            """{"v":1,"kind":"remote","url":"https://hawk.example.com","service_key":"hs_1","key":"hk_1"}"""
        )
        val profile = payload.toProfile()
        assertEquals(ConnectionKind.Remote, profile.kind)
        assertEquals("hs_1", profile.serviceKey)
        assertEquals("hk_1", profile.appKey)
    }

    @Test
    fun `rejects unsupported version`() {
        val error = assertThrows(ConnectionException::class.java) {
            PairingPayload.decode("""{"v":2,"kind":"lan","url":"http://a","token":"t"}""")
        }
        assertEquals("二维码版本不支持，请升级 hawk Android 客户端", error.message)
    }

    @Test
    fun `rejects lan payload without token`() {
        val error = assertThrows(ConnectionException::class.java) {
            PairingPayload.decode("""{"v":1,"kind":"lan","url":"http://a"}""").toProfile()
        }
        assertEquals("配对二维码缺少访问 token", error.message)
    }

    @Test
    fun `rejects non pairing content`() {
        val error = assertThrows(ConnectionException::class.java) {
            PairingPayload.decode("https://example.com")
        }
        assertEquals("这不是 hawk 的配对二维码", error.message)
    }
}
