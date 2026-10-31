package com.ryouji.hawk.data.connection

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class PairingPayloadTest {
    @Test
    fun `decodes unified payload without tenant key`() {
        val payload = PairingPayload.decode(
            """{"v":2,"target_library_id":"lib_1","target_device_id":"d_1","target_identity":"ep_1","access_key":"hk_1","lan_endpoints":["http://192.168.1.5:27375"],"wan":{"server_url":"https://hawk.example.com"}}"""
        )
        val profile = payload.toProfile()
        assertEquals(ConnectionKind.Remote, profile.kind)
        assertEquals("https://hawk.example.com", profile.url)
        assertEquals("hk_1", profile.token)
        assertEquals("ep_1", profile.targetIdentity)
        assertEquals("lib_1", profile.targetLibraryId)
        assertEquals("d_1", profile.targetDeviceId)
        assertEquals("hk_1", profile.appKey)
    }

    @Test
    fun `decodes lan only payload`() {
        val payload = PairingPayload.decode(
            """{"v":2,"target_library_id":"lib_1","target_identity":"ep_1","access_key":"hk_1","lan_endpoints":["http://192.168.1.5:27375"]}"""
        )
        val profile = payload.toProfile()
        assertEquals(ConnectionKind.Lan, profile.kind)
        assertEquals("http://192.168.1.5:27375", profile.url)
        assertEquals("ep_1", profile.targetIdentity)
        assertEquals("", profile.targetDeviceId)
    }

    @Test
    fun `rejects unsupported version`() {
        val error = assertThrows(ConnectionException::class.java) {
            PairingPayload.decode("""{"v":1,"kind":"lan","url":"http://a","token":"t"}""")
        }
        assertEquals("二维码版本不支持，请升级 hawk Android 客户端", error.message)
    }

    @Test
    fun `rejects missing identity`() {
        val error = assertThrows(ConnectionException::class.java) {
            PairingPayload.decode("""{"v":2,"target_identity":"","access_key":"hk_1","lan_endpoints":["http://a"]}""").toProfile()
        }
        assertEquals("配对二维码缺少目标身份或 AccessKey", error.message)
    }

    @Test
    fun `rejects non pairing content`() {
        val error = assertThrows(ConnectionException::class.java) {
            PairingPayload.decode("https://example.com")
        }
        assertEquals("这不是 hawk 的配对二维码", error.message)
    }
}
