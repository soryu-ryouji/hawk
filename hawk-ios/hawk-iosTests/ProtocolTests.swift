import XCTest

@testable import hawk_ios

nonisolated final class PairingPayloadTests: XCTestCase {
    func testDecodesV3AndDeduplicatesLanCandidates() throws {
        let payload = """
            {
              "v": 3,
              "target_library_id": "library-1",
              "access_key": "hk_secret",
              "lan_endpoints": [
                "http://192.168.1.5:27375",
                "http://192.168.1.5:27375",
                "http://10.0.0.5:27375"
              ]
            }
            """
        let profile = try PairingPayload.decode(payload).profile()
        XCTAssertEqual(profile.kind, .lan)
        XCTAssertEqual(profile.url, "http://192.168.1.5:27375")
        XCTAssertEqual(profile.accessKey, "hk_secret")
        XCTAssertEqual(profile.urls?.count, 2)
        XCTAssertEqual(try profile.candidateBaseURLs().count, 2)
    }

    func testDefaultsMissingSchemeToHTTP() throws {
        let payload = """
            {
              "v": 3,
              "access_key": "hk_lan",
              "lan_endpoints": ["192.168.1.8:27375"]
            }
            """
        let profile = try PairingPayload.decode(payload).profile()
        XCTAssertEqual(try profile.baseURL().absoluteString, "http://192.168.1.8:27375")
    }

    func testRejectsMissingAccessKeyAndEndpoints() {
        let missingKey = """
            {
              "v": 3,
              "lan_endpoints": ["http://192.168.1.8:27375"]
            }
            """
        XCTAssertThrowsError(try PairingPayload.decode(missingKey).profile()) { error in
            XCTAssertEqual(
                (error as? ConnectionError)?.errorDescription,
                "配对二维码缺少 AccessKey")
        }
        let missingEndpoint = """
            {
              "v": 3,
              "access_key": "hk_secret"
            }
            """
        XCTAssertThrowsError(try PairingPayload.decode(missingEndpoint).profile()) { error in
            XCTAssertEqual(
                (error as? ConnectionError)?.errorDescription,
                "配对二维码缺少局域网地址")
        }
    }

    func testRejectsUnsupportedVersion() {
        let unsupported = """
            {
              "v": 2,
              "access_key": "hk_secret",
              "lan_endpoints": ["http://192.168.1.8:27375"]
            }
            """
        XCTAssertThrowsError(try PairingPayload.decode(unsupported)) { error in
            XCTAssertEqual(
                (error as? ConnectionError)?.errorDescription,
                "二维码版本不支持，请升级 hawk iOS 客户端")
        }
    }
}

nonisolated final class APIContractTests: XCTestCase {
    func testMediaAndEventRequestsUseBearerWithoutCredentialInURL() throws {
        let api = APIClient(
            baseURL: try XCTUnwrap(URL(string: "http://127.0.0.1:32123")),
            credentials: SessionCredentials(token: "access-key"))
        let request = try XCTUnwrap(api.eventRequest())
        XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer access-key")
        XCTAssertNil(request.value(forHTTPHeaderField: "Cookie"))
        XCTAssertEqual(request.url?.path, "/api/v1/events")
        XCTAssertEqual(request.url?.query?.contains("access-key"), false)

        let media = try XCTUnwrap(api.fileURL(id: "item/with space"))
        XCTAssertEqual(media.query?.contains("access-key"), false)
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

nonisolated final class LanConnectionInfoTests: XCTestCase {
    func testResolvesProfileFromV3ConnectionInfo() throws {
        let data = Data(
            #"{"protocol":3,"peer_id":"peer-1","name":"desktop","library_name":"照片库","peer_port":27375,"target_library_id":"library-lan"}"#
                .utf8)
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let info = try decoder.decode(LanConnectionInfo.self, from: data)
        XCTAssertEqual(info.protocol, LanConnectionInfo.currentProtocol)
        XCTAssertEqual(info.peerId, "peer-1")
        XCTAssertEqual(info.name, "desktop")
        XCTAssertEqual(info.libraryName, "照片库")
        XCTAssertEqual(info.peerPort, 27375)
        XCTAssertEqual(info.targetLibraryId, "library-lan")

        let baseURL = try XCTUnwrap(URL(string: "http://192.168.1.8:27375"))
        let profile = ConnectionProfile(
            kind: .lan, url: baseURL.absoluteString, accessKey: "hk_lan"
        ).resolving(with: info, at: baseURL)
        XCTAssertEqual(profile.name, "照片库")
        XCTAssertEqual(profile.url, baseURL.absoluteString)
        XCTAssertEqual(profile.accessKey, "hk_lan")
    }

    func testResolvingKeepsExistingProfileName() throws {
        let data = Data(
            #"{"protocol":3,"peer_id":"peer-1","name":"desktop","library_name":"照片库","peer_port":27375,"target_library_id":"library-lan"}"#
                .utf8)
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let info = try decoder.decode(LanConnectionInfo.self, from: data)
        let baseURL = try XCTUnwrap(URL(string: "http://192.168.1.8:27375"))
        let profile = ConnectionProfile(
            kind: .lan, name: "书房桌面", url: "http://192.168.1.8:27375", accessKey: "hk_lan"
        ).resolving(with: info, at: baseURL)
        XCTAssertEqual(profile.name, "书房桌面")
    }
}
