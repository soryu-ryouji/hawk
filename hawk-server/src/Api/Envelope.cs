using System.Text.Json;
using Microsoft.AspNetCore.Http.Json;
using Microsoft.Extensions.Options;

namespace Hawk.Server.Api;

/// <summary>统一成功信封；data 为空时省略该字段</summary>
public sealed record Envelope<T>(string Status, T? Data)
{
    public static Envelope<T> Ok(T data) => new("success", data);
}

/// <summary>统一错误信封</summary>
public sealed record ErrorEnvelope(string Status, ErrorBody Error);

public sealed record ErrorBody(string Code, string Message);

/// <summary>API 错误码（见 server-rest-api-v1.md）</summary>
public static class ErrorCodes
{
    public const string InvalidParam = "INVALID_PARAM";
    public const string ItemNotFound = "ITEM_NOT_FOUND";
    public const string FolderNotFound = "FOLDER_NOT_FOUND";
    public const string FileExists = "FILE_EXISTS";
    public const string UnsupportedFormat = "UNSUPPORTED_FORMAT";
    public const string Internal = "INTERNAL";
}

/// <summary>携带错误码与 HTTP 状态的业务异常，由中间件统一转为错误信封</summary>
public sealed class ApiException : Exception
{
    public string Code { get; }
    public int HttpStatus { get; }

    public ApiException(string code, string message, int httpStatus) : base(message)
    {
        Code = code;
        HttpStatus = httpStatus;
    }

    public static ApiException InvalidParam(string message) => new(ErrorCodes.InvalidParam, message, StatusCodes.Status400BadRequest);
    public static ApiException ItemNotFound(string id) => new(ErrorCodes.ItemNotFound, $"item {id} not found", StatusCodes.Status404NotFound);
    public static ApiException FolderNotFound(string path) => new(ErrorCodes.FolderNotFound, $"folder {path} not found", StatusCodes.Status404NotFound);
    public static ApiException FileExists(string path) => new(ErrorCodes.FileExists, $"file already exists: {path}", StatusCodes.Status409Conflict);
    public static ApiException UnsupportedFormat(string message) => new(ErrorCodes.UnsupportedFormat, message, StatusCodes.Status400BadRequest);
}

/// <summary>异常 → 统一错误信封</summary>
public sealed class ErrorHandlingMiddleware
{
    private readonly RequestDelegate _next;
    private readonly ILogger<ErrorHandlingMiddleware> _logger;

    public ErrorHandlingMiddleware(RequestDelegate next, ILogger<ErrorHandlingMiddleware> logger)
    {
        _next = next;
        _logger = logger;
    }

    public async Task Invoke(HttpContext context, IOptions<JsonOptions> jsonOptions)
    {
        try
        {
            await _next(context);
        }
        catch (ApiException ex)
        {
            await WriteError(context, jsonOptions, ex.HttpStatus, ex.Code, ex.Message);
        }
        catch (BadHttpRequestException ex)
        {
            await WriteError(context, jsonOptions, StatusCodes.Status400BadRequest, ErrorCodes.InvalidParam, ex.Message);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "未处理的请求异常");
            await WriteError(context, jsonOptions, StatusCodes.Status500InternalServerError, ErrorCodes.Internal, ex.Message);
        }
    }

    private static async Task WriteError(HttpContext context, IOptions<JsonOptions> jsonOptions, int status, string code, string message)
    {
        if (context.Response.HasStarted)
        {
            return;
        }

        context.Response.StatusCode = status;
        var body = new ErrorEnvelope("error", new ErrorBody(code, message));
        await context.Response.WriteAsJsonAsync(body, jsonOptions.Value.SerializerOptions);
    }
}
