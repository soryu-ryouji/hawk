import XCTest

@testable import hawk_ios

nonisolated final class PairingPayloadTests: XCTestCase {
    func testDecodesRemoteV2AndKeepsLanCandidates() throws {
        let payload = """
            {
              "v": 2,
              "target_library_id": "library-1",
              "target_device_id": "device-1",
              "target_identity": "endpoint-1",
              "access_key": "hk_secret",
              "lan_endpoints": [
                "http://192.168.1.5:27375",
                "http://192.168.1.5:27375",
                "http://10.0.0.5:27375"
              ],
              "wan": { "server_url": "https://hawk.example.com" }
            }
            """
        let profile = try PairingPayload.decode(payload).profile()
        XCTAssertEqual(profile.kind, .remote)
        XCTAssertEqual(profile.url, "https://hawk.example.com")
        XCTAssertEqual(profile.accessKey, "hk_secret")
        XCTAssertEqual(profile.targetIdentity, "endpoint-1")
        XCTAssertEqual(profile.targetDeviceId, "device-1")
        XCTAssertEqual(profile.targetLibraryId, "library-1")
        XCTAssertEqual(profile.urls?.count, 2)
        XCTAssertEqual(try profile.candidateBaseURLs().count, 3)
    }

    func testDecodesLanV2IdentityAndAccessKey() throws {
        let payload = """
            {
              "v": 2,
              "target_identity": "endpoint-lan",
              "access_key": "hk_lan",
              "lan_endpoints": ["http://192.168.1.8:27375"]
            }
            """
        let profile = try PairingPayload.decode(payload).profile()
        XCTAssertEqual(profile.kind, .lan)
        XCTAssertEqual(profile.targetIdentity, "endpoint-lan")
        XCTAssertEqual(profile.accessKey, "hk_lan")
        XCTAssertEqual(profile.targetDeviceId, "")
    }

    func testRejectsUnsupportedVersionAndMissingWanDevice() {
        let unsupported = """
            {
              "v": 1,
              "target_identity": "endpoint-1",
              "access_key": "hk_secret",
              "lan_endpoints": ["http://192.168.1.8:27375"]
            }
            """
        XCTAssertThrowsError(try PairingPayload.decode(unsupported)) { error in
            XCTAssertEqual(
                (error as? ConnectionError)?.errorDescription,
                "二维码版本不支持，请升级 hawk iOS 客户端")
        }
        let missingDevice = """
            {
              "v": 2,
              "target_identity": "endpoint-1",
              "access_key": "hk_secret",
              "lan_endpoints": [],
              "wan": { "server_url": "https://hawk.example.com" }
            }
            """
        XCTAssertThrowsError(try PairingPayload.decode(missingDevice).profile())
    }
}

nonisolated final class APIContractTests: XCTestCase {
    func testMediaAndEventRequestsUseBearerWithoutCredentialInURL() throws {
        let api = APIClient(
            baseURL: try XCTUnwrap(URL(string: "http://127.0.0.1:32123")),
            credentials: SessionCredentials(token: "session-secret"))
        let request = try XCTUnwrap(api.eventRequest())
        XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer session-secret")
        XCTAssertNil(request.value(forHTTPHeaderField: "Cookie"))
        XCTAssertEqual(request.url?.path, "/api/v1/events")
        XCTAssertEqual(request.url?.query?.contains("session-secret"), false)

        let media = try XCTUnwrap(api.fileURL(id: "item/with space"))
        XCTAssertEqual(media.query?.contains("session-secret"), false)
        XCTAssertEqual(media.query?.contains("unlock="), false)
    }

    func testItemListRequestUsesCurrentSnakeCaseFields() throws {
        var request = ItemListRequest()
        request.categoriesMatch = "any"
        request.minSideGte = 720
        request.minWidth = 1280
        request.maxHeight = 4096
        request.foldersExact = true
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        let object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: encoder.encode(request)) as? [String: Any])
        XCTAssertEqual(object["categories_match"] as? String, "any")
        XCTAssertEqual(object["min_side_gte"] as? Int, 720)
        XCTAssertEqual(object["min_width"] as? Int, 1280)
        XCTAssertEqual(object["max_height"] as? Int, 4096)
        XCTAssertEqual(object["folders_exact"] as? Bool, true)
    }

    func testLibraryScanIntervalIsDecodedInSeconds() throws {
        let data = Data(#"{"periodic":true,"interval":60}"#.utf8)
        let scan = try JSONDecoder().decode(ScanInfo.self, from: data)
        XCTAssertTrue(scan.periodic)
        XCTAssertEqual(scan.interval, 60)
    }
}

nonisolated final class RustTransportContractTests: XCTestCase {
    func testDecodesSuccessfulConnection() throws {
        let data = Data(
            #"{"session_id":"s1","api_base":"http://127.0.0.1:3000","credential":"c1","role":"importer","target_device_id":"d1"}"#
                .utf8)
        let connection = try RustTransport.decodeConnection(data)
        XCTAssertEqual(connection.sessionId, "s1")
        XCTAssertEqual(connection.credential, "c1")
        XCTAssertEqual(connection.role, "importer")
    }

    func testPreservesNativeErrorMessage() {
        let data = Data(#"{"error":"目标身份与配对信息不一致"}"#.utf8)
        XCTAssertThrowsError(try RustTransport.decodeConnection(data)) { error in
            XCTAssertEqual((error as? RustTransportError)?.message, "目标身份与配对信息不一致")
        }
    }
}
