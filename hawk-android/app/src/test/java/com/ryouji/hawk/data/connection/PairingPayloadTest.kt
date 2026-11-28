package com.ryouji.hawk.data.connection

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class PairingPayloadTest {
    @Test
    fun `decodes v3 lan payload`() {
        val payload = PairingPayload.decode(
            """{"v":3,"target_library_id":"lib_1","access_key":"hk_1","lan_endpoints":["http://192.168.1.5:27375","http://192.168.1.6:27375"]}"""
        )
        val profile = payload.toProfile()
        assertEquals(ConnectionKind.Lan, profile.kind)
        assertEquals("http://192.168.1.5:27375", profile.url)
        assertEquals(
            listOf("http://192.168.1.5:27375", "http://192.168.1.6:27375"),
            profile.urls,
        )
        assertEquals("hk_1", profile.accessKey)
    }

    @Test
    fun `decodes payload without library id`() {
        val payload = PairingPayload.decode(
            """{"v":3,"access_key":"hk_1","lan_endpoints":["192.168.1.5:27375"]}"""
        )
        val profile = payload.toProfile()
        assertEquals(ConnectionKind.Lan, profile.kind)
        assertEquals("192.168.1.5:27375", profile.url)
        assertEquals("hk_1", profile.accessKey)
    }

    @Test
    fun `rejects unsupported version`() {
        val error = assertThrows(ConnectionException::class.java) {
            PairingPayload.decode("""{"v":2,"access_key":"hk_1","lan_endpoints":["http://a"]}""")
        }
        assertEquals("二维码版本不支持，请升级 hawk Android 客户端", error.message)
    }

    @Test
    fun `rejects missing access key`() {
        val error = assertThrows(ConnectionException::class.java) {
            PairingPayload.decode("""{"v":3,"access_key":"","lan_endpoints":["http://a"]}""").toProfile()
        }
        assertEquals("配对二维码缺少访问密钥", error.message)
    }

    @Test
    fun `rejects missing endpoints`() {
        val error = assertThrows(ConnectionException::class.java) {
            PairingPayload.decode("""{"v":3,"access_key":"hk_1","lan_endpoints":[]}""").toProfile()
        }
        assertEquals("配对二维码缺少局域网地址", error.message)
    }

    @Test
    fun `rejects non pairing content`() {
        val error = assertThrows(ConnectionException::class.java) {
            PairingPayload.decode("https://example.com")
        }
        assertEquals("这不是 hawk 的配对二维码", error.message)
    }
}
