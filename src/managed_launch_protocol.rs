//! Bounded borrowed wire codec for the experimental managed-launch request.
//!
//! This module validates and encodes schema-6 launch and status payloads plus
//! launch results. It performs no filesystem, process, socket, persistence, or
//! dispatch work. Request decoding borrows all returned byte fields; duplicate
//! environment-name validation temporarily stores at most 4,096 borrowed names
//! in a bounded set after the complete-byte and count checks. The codec does
//! not authorize any operation.

use crate::control_protocol::TargetRef;
use crate::protocol::{OperationClass, OperationKey, Outcome, MAX_OPERATION_PAYLOAD_BYTES};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::io::{self, Write};
use std::iter::FusedIterator;

/// Wire schema used for the experimental managed-launch payloads.
pub const MANAGED_LAUNCH_SCHEMA: u16 = 6;

/// Maximum complete encoded launch request, including its prefix.
pub const MAX_LAUNCH_REQUEST_BYTES: usize = MAX_OPERATION_PAYLOAD_BYTES;

/// Maximum number of argument byte strings in one launch request.
pub const MAX_ARGUMENT_COUNT: usize = 256;

/// Maximum number of environment entries in one launch request.
pub const MAX_ENVIRONMENT_COUNT: usize = 4_096;

/// Maximum raw byte length for each of the three launch paths.
pub const MAX_PATH_BYTES: usize = 4_096;

const REQUEST_PREFIX_BYTES: usize = 64;
const STATUS_QUERY_BYTES: usize = 32;
const RESULT_BYTES: usize = 80;
const KIND_LAUNCH: u16 = 1;
const KIND_STATUS_QUERY: u16 = 2;
const KIND_RESULT: u16 = 0x8001;
const RESERVED_ENVIRONMENT_PREFIX: &[u8] = b"PANELINK_";

/// One borrowed environment name/value pair. Ordering is preserved as supplied.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct EnvironmentEntry<'a> {
    /// Raw environment name bytes.
    pub name: &'a [u8],
    /// Raw environment value bytes.
    pub value: &'a [u8],
}

impl fmt::Debug for EnvironmentEntry<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnvironmentEntry { <redacted> }")
    }
}

/// Borrowed typed schema-6 managed-launch request.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LaunchRequest<'a> {
    /// Existing generation/lease/sequence key used by operation admission.
    pub key: OperationKey,
    /// Requested initial terminal row count.
    pub rows: u16,
    /// Requested initial terminal column count.
    pub columns: u16,
    /// Requested executable as raw Unix path or basename bytes.
    pub requested_executable: &'a [u8],
    /// Requested working directory as raw path bytes.
    pub requested_working_directory: &'a [u8],
    /// Captured client working directory as raw absolute path bytes.
    pub client_working_directory: &'a [u8],
    /// Borrowed argument byte strings in transmitted order.
    pub arguments: &'a [&'a [u8]],
    /// Borrowed environment entries in transmitted order.
    pub environment: &'a [EnvironmentEntry<'a>],
}

impl LaunchRequest<'_> {
    /// Classify this request under the existing operation protocol.
    pub const fn operation_class(&self) -> OperationClass {
        OperationClass::DurableMutation
    }

    /// Validate every field and return the complete encoded length.
    pub fn encoded_len(&self) -> Result<usize, CodecError> {
        validate_launch_request(self)
    }
}

impl fmt::Debug for LaunchRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LaunchRequest { <redacted> }")
    }
}

/// A decoded launch request borrowing all byte fields from the input payload.
///
/// Argument and environment views are parsed lazily from the validated encoded
/// body, so the returned view allocates no vectors or copies of caller bytes.
/// Validation uses a temporary set with at most 4,096 borrowed environment
/// names to check uniqueness; complete-size and count checks happen first.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LaunchRequestView<'a> {
    key: OperationKey,
    rows: u16,
    columns: u16,
    requested_executable: &'a [u8],
    requested_working_directory: &'a [u8],
    client_working_directory: &'a [u8],
    argument_bytes: &'a [u8],
    argument_count: u16,
    environment_bytes: &'a [u8],
    environment_count: u16,
    encoded_len: usize,
}

impl<'a> LaunchRequestView<'a> {
    /// Return the validated operation key.
    pub const fn key(&self) -> OperationKey {
        self.key
    }

    /// Return validated initial terminal dimensions as `(rows, columns)`.
    pub const fn dimensions(&self) -> (u16, u16) {
        (self.rows, self.columns)
    }

    /// Return the requested executable bytes without UTF-8 conversion.
    pub const fn requested_executable(&self) -> &'a [u8] {
        self.requested_executable
    }

    /// Return the requested working-directory bytes without UTF-8 conversion.
    pub const fn requested_working_directory(&self) -> &'a [u8] {
        self.requested_working_directory
    }

    /// Return the captured absolute client working-directory bytes.
    pub const fn client_working_directory(&self) -> &'a [u8] {
        self.client_working_directory
    }

    /// Iterate over borrowed arguments in their encoded order.
    pub fn arguments(&self) -> ArgumentIter<'a> {
        ArgumentIter {
            cursor: Cursor::new(self.argument_bytes),
            remaining: self.argument_count,
        }
    }

    /// Iterate over borrowed environment entries in their encoded order.
    pub fn environment(&self) -> EnvironmentIter<'a> {
        EnvironmentIter {
            cursor: Cursor::new(self.environment_bytes),
            remaining: self.environment_count,
        }
    }

    /// Return the exact encoded length validated by the decoder.
    pub const fn encoded_len(&self) -> usize {
        self.encoded_len
    }
}

impl fmt::Debug for LaunchRequestView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LaunchRequestView { <redacted> }")
    }
}

/// Iterator over the argument section of a validated launch request.
pub struct ArgumentIter<'a> {
    cursor: Cursor<'a>,
    remaining: u16,
}

impl<'a> Iterator for ArgumentIter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let length = self
            .cursor
            .read_u32()
            .expect("validated argument length is present") as usize;
        let value = self
            .cursor
            .take(length)
            .expect("validated argument bytes are present");
        self.remaining -= 1;
        Some(value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = usize::from(self.remaining);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for ArgumentIter<'_> {}
impl FusedIterator for ArgumentIter<'_> {}

/// Iterator over environment entries in a validated launch request.
pub struct EnvironmentIter<'a> {
    cursor: Cursor<'a>,
    remaining: u16,
}

impl<'a> Iterator for EnvironmentIter<'a> {
    type Item = EnvironmentEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let name_length =
            self.cursor
                .read_u16()
                .expect("validated environment name length is present") as usize;
        let value_length =
            self.cursor
                .read_u32()
                .expect("validated environment value length is present") as usize;
        let name = self
            .cursor
            .take(name_length)
            .expect("validated environment name is present");
        let value = self
            .cursor
            .take(value_length)
            .expect("validated environment value is present");
        self.remaining -= 1;
        Some(EnvironmentEntry { name, value })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = usize::from(self.remaining);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for EnvironmentIter<'_> {}
impl FusedIterator for EnvironmentIter<'_> {}

/// A status query for the original launch operation key.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LaunchStatusQuery {
    /// Original operation key whose result is observed.
    pub key: OperationKey,
}

impl LaunchStatusQuery {
    /// Classify status as an observation that does not reserve a new operation.
    pub const fn operation_class(self) -> OperationClass {
        OperationClass::Query
    }

    /// Validate the operation key and return the fixed encoded size.
    pub fn encoded_len(self) -> Result<usize, CodecError> {
        validate_key(self.key)?;
        Ok(STATUS_QUERY_BYTES)
    }
}

impl fmt::Debug for LaunchStatusQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LaunchStatusQuery { <redacted> }")
    }
}

/// Typed result for one managed launch operation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LaunchResult {
    /// Existing key identifying the launch operation.
    pub key: OperationKey,
    /// Existing operation outcome; `Partial` is not valid on this wire schema.
    pub outcome: Outcome,
    /// Reserved full target identity when the result requires one.
    pub target: Option<TargetRef>,
}

impl LaunchResult {
    /// Validate this result and return its fixed encoded size.
    pub fn encoded_len(&self) -> Result<usize, CodecError> {
        validate_result(self)?;
        Ok(RESULT_BYTES)
    }
}

impl fmt::Debug for LaunchResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LaunchResult { <redacted> }")
    }
}

/// Fixed, redacted validation errors for managed-launch payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecError {
    /// The complete payload exceeds the operation limit.
    PayloadTooLarge,
    /// A required prefix or length-delimited field is truncated.
    Truncated,
    /// Bytes remain after the declared body.
    TrailingBytes,
    /// A checked byte-count calculation overflowed.
    LengthOverflow,
    /// The schema is unsupported.
    UnsupportedSchema,
    /// The request or result kind is unknown.
    UnknownKind,
    /// A reserved field contains a nonzero byte.
    NonzeroReserved,
    /// An operation-key component is zero.
    ZeroKeyComponent,
    /// Terminal dimensions are outside the supported bounds.
    InvalidDimensions,
    /// There are too many arguments.
    TooManyArguments,
    /// There are too many environment entries.
    TooManyEnvironmentEntries,
    /// A path is empty or exceeds its raw-byte limit.
    InvalidPathLength,
    /// A requested executable is neither absolute nor a slash-free basename.
    InvalidExecutable,
    /// The captured client working directory is not absolute.
    ClientWorkingDirectoryNotAbsolute,
    /// A NUL byte occurs in a path, argument, environment name, or value.
    NulByte,
    /// An environment name is empty or contains `=`.
    InvalidEnvironmentName,
    /// An environment name is duplicated.
    DuplicateEnvironmentName,
    /// An environment name uses the reserved `PANELINK_` prefix.
    ReservedEnvironmentName,
    /// An outcome code is unsupported by managed launch.
    UnknownOutcome,
    /// A target component is zero or its generation does not match the key.
    InvalidTarget,
    /// The target-presence flag is not zero or one.
    InvalidTargetPresence,
    /// The target presence does not match the outcome contract.
    OutcomeTargetMismatch,
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::PayloadTooLarge => "managed-launch payload exceeds its limit",
            Self::Truncated => "managed-launch payload is truncated",
            Self::TrailingBytes => "bytes follow the managed-launch payload",
            Self::LengthOverflow => "managed-launch length calculation overflowed",
            Self::UnsupportedSchema => "managed-launch schema is unsupported",
            Self::UnknownKind => "managed-launch kind is unknown",
            Self::NonzeroReserved => "managed-launch reserved bytes must be zero",
            Self::ZeroKeyComponent => "managed-launch key components must be nonzero",
            Self::InvalidDimensions => "managed-launch dimensions are outside their limits",
            Self::TooManyArguments => "managed-launch argument count exceeds its limit",
            Self::TooManyEnvironmentEntries => "managed-launch environment count exceeds its limit",
            Self::InvalidPathLength => "managed-launch path length is invalid",
            Self::InvalidExecutable => "managed-launch executable form is invalid",
            Self::ClientWorkingDirectoryNotAbsolute => {
                "managed-launch client working directory must be absolute"
            }
            Self::NulByte => "managed-launch byte fields cannot contain NUL",
            Self::InvalidEnvironmentName => "managed-launch environment name is invalid",
            Self::DuplicateEnvironmentName => "managed-launch environment names must be unique",
            Self::ReservedEnvironmentName => {
                "managed-launch environment name uses a reserved prefix"
            }
            Self::UnknownOutcome => "managed-launch outcome is unsupported",
            Self::InvalidTarget => "managed-launch target is invalid",
            Self::InvalidTargetPresence => "managed-launch target flag is invalid",
            Self::OutcomeTargetMismatch => "managed-launch outcome and target do not match",
        };
        f.write_str(message)
    }
}

impl Error for CodecError {}

/// Encode failures preserve a writer's original `io::Error` while keeping the
/// codec's own debug and display text free of caller-supplied launch data.
pub enum EncodeError {
    /// Validation failed before the encoder wrote any bytes.
    Invalid(CodecError),
    /// The destination writer rejected some encoded bytes.
    Writer(io::Error),
}

impl fmt::Debug for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(error) => f.debug_tuple("EncodeError::Invalid").field(error).finish(),
            Self::Writer(_) => f.write_str("EncodeError::Writer(<redacted>)"),
        }
    }
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(_) => f.write_str("managed-launch codec validation failed"),
            Self::Writer(_) => f.write_str("managed-launch codec writer failed"),
        }
    }
}

impl Error for EncodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Invalid(error) => Some(error),
            Self::Writer(error) => Some(error),
        }
    }
}

impl From<CodecError> for EncodeError {
    fn from(value: CodecError) -> Self {
        Self::Invalid(value)
    }
}

/// Decode and validate exactly one complete schema-6 launch request.
pub fn decode_launch_request(bytes: &[u8]) -> Result<LaunchRequestView<'_>, CodecError> {
    validate_total_len(bytes.len())?;
    if bytes.len() < REQUEST_PREFIX_BYTES {
        return Err(CodecError::Truncated);
    }
    validate_header(bytes, KIND_LAUNCH)?;

    let key = read_key(bytes, 4)?;
    validate_key(key)?;
    let rows = read_u16(bytes, 28)?;
    let columns = read_u16(bytes, 30)?;
    validate_dimensions(rows, columns)?;
    let argument_count = read_u16(bytes, 32)?;
    let environment_count = read_u16(bytes, 34)?;
    validate_counts(argument_count as usize, environment_count as usize)?;
    let executable_length = read_u32_as_usize(bytes, 36)?;
    let requested_cwd_length = read_u32_as_usize(bytes, 40)?;
    let client_cwd_length = read_u32_as_usize(bytes, 44)?;
    validate_path_length(executable_length)?;
    validate_path_length(requested_cwd_length)?;
    validate_path_length(client_cwd_length)?;
    if bytes[48..REQUEST_PREFIX_BYTES]
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(CodecError::NonzeroReserved);
    }

    // Reject impossible count/length combinations before constructing borrowed
    // views or allocating the bounded environment-name uniqueness set.
    let path_bytes = executable_length
        .checked_add(requested_cwd_length)
        .and_then(|value| value.checked_add(client_cwd_length))
        .ok_or(CodecError::LengthOverflow)?;
    let minimum_arguments = usize::from(argument_count)
        .checked_mul(4)
        .ok_or(CodecError::LengthOverflow)?;
    let minimum_environment = usize::from(environment_count)
        .checked_mul(6)
        .ok_or(CodecError::LengthOverflow)?;
    let minimum_length = REQUEST_PREFIX_BYTES
        .checked_add(path_bytes)
        .and_then(|value| value.checked_add(minimum_arguments))
        .and_then(|value| value.checked_add(minimum_environment))
        .ok_or(CodecError::LengthOverflow)?;
    if bytes.len() < minimum_length {
        return Err(CodecError::Truncated);
    }

    let mut cursor = Cursor::with_offset(bytes, REQUEST_PREFIX_BYTES);
    let requested_executable = cursor.take(executable_length)?;
    let requested_working_directory = cursor.take(requested_cwd_length)?;
    let client_working_directory = cursor.take(client_cwd_length)?;
    validate_path(requested_executable)?;
    validate_executable(requested_executable)?;
    validate_path(requested_working_directory)?;
    validate_path(client_working_directory)?;
    validate_client_cwd(client_working_directory)?;

    let argument_start = cursor.offset;
    for _ in 0..argument_count {
        let argument_length = cursor.read_u32_as_usize()?;
        let argument = cursor.take(argument_length)?;
        validate_no_nul(argument)?;
    }
    let environment_start = cursor.offset;
    for _ in 0..environment_count {
        let name_length = usize::from(cursor.read_u16()?);
        let value_length = cursor.read_u32_as_usize()?;
        let name = cursor.take(name_length)?;
        let value = cursor.take(value_length)?;
        validate_environment_entry(name, value)?;
    }
    if cursor.offset != bytes.len() {
        return Err(CodecError::TrailingBytes);
    }
    let argument_end = environment_start;
    let environment_end = cursor.offset;
    validate_unique_encoded_environment(
        &bytes[environment_start..environment_end],
        environment_count,
    )?;

    Ok(LaunchRequestView {
        key,
        rows,
        columns,
        requested_executable,
        requested_working_directory,
        client_working_directory,
        argument_bytes: &bytes[argument_start..argument_end],
        argument_count,
        environment_bytes: &bytes[environment_start..environment_end],
        environment_count,
        encoded_len: bytes.len(),
    })
}

/// Validate and encode one launch request before the first writer call.
pub fn encode_launch_request<W: Write>(
    request: &LaunchRequest<'_>,
    writer: &mut W,
) -> Result<usize, EncodeError> {
    let encoded_len = request.encoded_len()?;
    write_u16(writer, MANAGED_LAUNCH_SCHEMA)?;
    write_u16(writer, KIND_LAUNCH)?;
    write_key(writer, request.key)?;
    write_u16(writer, request.rows)?;
    write_u16(writer, request.columns)?;
    write_u16(writer, request.arguments.len() as u16)?;
    write_u16(writer, request.environment.len() as u16)?;
    write_u32(writer, request.requested_executable.len() as u32)?;
    write_u32(writer, request.requested_working_directory.len() as u32)?;
    write_u32(writer, request.client_working_directory.len() as u32)?;
    writer.write_all(&[0; 16]).map_err(EncodeError::Writer)?;
    writer
        .write_all(request.requested_executable)
        .map_err(EncodeError::Writer)?;
    writer
        .write_all(request.requested_working_directory)
        .map_err(EncodeError::Writer)?;
    writer
        .write_all(request.client_working_directory)
        .map_err(EncodeError::Writer)?;
    for argument in request.arguments {
        write_u32(writer, argument.len() as u32)?;
        writer.write_all(argument).map_err(EncodeError::Writer)?;
    }
    for entry in request.environment {
        write_u16(writer, entry.name.len() as u16)?;
        write_u32(writer, entry.value.len() as u32)?;
        writer.write_all(entry.name).map_err(EncodeError::Writer)?;
        writer.write_all(entry.value).map_err(EncodeError::Writer)?;
    }
    Ok(encoded_len)
}

/// Validate and encode a fixed-size query for an already-admitted launch key.
pub fn encode_status_query<W: Write>(
    query: LaunchStatusQuery,
    writer: &mut W,
) -> Result<usize, EncodeError> {
    let encoded_len = query.encoded_len()?;
    write_u16(writer, MANAGED_LAUNCH_SCHEMA)?;
    write_u16(writer, KIND_STATUS_QUERY)?;
    write_key(writer, query.key)?;
    writer.write_all(&[0; 4]).map_err(EncodeError::Writer)?;
    Ok(encoded_len)
}

/// Decode one exact 32-byte status query without reserving an operation key.
pub fn decode_status_query(bytes: &[u8]) -> Result<LaunchStatusQuery, CodecError> {
    validate_exact_size(bytes, STATUS_QUERY_BYTES)?;
    validate_header(bytes, KIND_STATUS_QUERY)?;
    let key = read_key(bytes, 4)?;
    validate_key(key)?;
    if bytes[28..32].iter().any(|byte| *byte != 0) {
        return Err(CodecError::NonzeroReserved);
    }
    Ok(LaunchStatusQuery { key })
}

/// Validate and encode one fixed-size managed-launch result.
pub fn encode_launch_result<W: Write>(
    result: &LaunchResult,
    writer: &mut W,
) -> Result<usize, EncodeError> {
    let encoded_len = result.encoded_len()?;
    write_u16(writer, MANAGED_LAUNCH_SCHEMA)?;
    write_u16(writer, KIND_RESULT)?;
    write_key(writer, result.key)?;
    write_u16(writer, outcome_code(result.outcome)?)?;
    match result.target {
        Some(target) => {
            write_u16(writer, 1)?;
            write_target(writer, target)?;
        }
        None => {
            write_u16(writer, 0)?;
            writer.write_all(&[0; 40]).map_err(EncodeError::Writer)?;
        }
    }
    writer.write_all(&[0; 8]).map_err(EncodeError::Writer)?;
    Ok(encoded_len)
}

/// Decode one exact 80-byte managed-launch result.
pub fn decode_launch_result(bytes: &[u8]) -> Result<LaunchResult, CodecError> {
    validate_exact_size(bytes, RESULT_BYTES)?;
    validate_header(bytes, KIND_RESULT)?;
    let key = read_key(bytes, 4)?;
    validate_key(key)?;
    let outcome = decode_outcome(read_u16(bytes, 28)?)?;
    let target_flag = read_u16(bytes, 30)?;
    let target_values = [
        read_u64(bytes, 32)?,
        read_u64(bytes, 40)?,
        read_u64(bytes, 48)?,
        read_u64(bytes, 56)?,
        read_u64(bytes, 64)?,
    ];
    let target = match target_flag {
        0 if target_values.iter().all(|value| *value == 0) => None,
        0 => return Err(CodecError::NonzeroReserved),
        1 => Some(TargetRef {
            generation: target_values[0],
            workspace: target_values[1],
            pane: target_values[2],
            session: target_values[3],
            occupant_revision: target_values[4],
        }),
        _ => return Err(CodecError::InvalidTargetPresence),
    };
    if bytes[72..80].iter().any(|byte| *byte != 0) {
        return Err(CodecError::NonzeroReserved);
    }
    let result = LaunchResult {
        key,
        outcome,
        target,
    };
    validate_result(&result)?;
    Ok(result)
}

fn validate_launch_request(request: &LaunchRequest<'_>) -> Result<usize, CodecError> {
    validate_key(request.key)?;
    validate_dimensions(request.rows, request.columns)?;
    validate_counts(request.arguments.len(), request.environment.len())?;
    validate_path(request.requested_executable)?;
    validate_executable(request.requested_executable)?;
    validate_path(request.requested_working_directory)?;
    validate_path(request.client_working_directory)?;
    validate_client_cwd(request.client_working_directory)?;
    for argument in request.arguments {
        validate_no_nul(argument)?;
    }
    for entry in request.environment {
        validate_environment_entry(entry.name, entry.value)?;
    }

    let mut encoded_len = REQUEST_PREFIX_BYTES;
    encoded_len = checked_add(encoded_len, request.requested_executable.len())?;
    encoded_len = checked_add(encoded_len, request.requested_working_directory.len())?;
    encoded_len = checked_add(encoded_len, request.client_working_directory.len())?;
    for argument in request.arguments {
        encoded_len = checked_add(encoded_len, 4)?;
        encoded_len = checked_add(encoded_len, argument.len())?;
    }
    for entry in request.environment {
        encoded_len = checked_add(encoded_len, 6)?;
        encoded_len = checked_add(encoded_len, entry.name.len())?;
        encoded_len = checked_add(encoded_len, entry.value.len())?;
    }
    validate_total_len(encoded_len)?;
    let mut names = BTreeSet::new();
    for entry in request.environment {
        if !names.insert(entry.name) {
            return Err(CodecError::DuplicateEnvironmentName);
        }
    }
    Ok(encoded_len)
}

fn validate_result(result: &LaunchResult) -> Result<(), CodecError> {
    validate_key(result.key)?;
    if let Some(target) = result.target {
        validate_target(result.key, target)?;
    }
    match (result.outcome, result.target.is_some()) {
        (Outcome::Applied, true)
        | (Outcome::Failed, false | true)
        | (Outcome::Accepted, true)
        | (Outcome::Unknown, true) => Ok(()),
        (Outcome::Partial, _) => Err(CodecError::UnknownOutcome),
        _ => Err(CodecError::OutcomeTargetMismatch),
    }
}

fn validate_target(key: OperationKey, target: TargetRef) -> Result<(), CodecError> {
    if target.generation == 0
        || target.workspace == 0
        || target.pane == 0
        || target.session == 0
        || target.occupant_revision == 0
        || target.generation != key.generation
    {
        return Err(CodecError::InvalidTarget);
    }
    Ok(())
}

fn validate_key(key: OperationKey) -> Result<(), CodecError> {
    if key.generation == 0 || key.lease == 0 || key.sequence == 0 {
        Err(CodecError::ZeroKeyComponent)
    } else {
        Ok(())
    }
}

fn validate_dimensions(rows: u16, columns: u16) -> Result<(), CodecError> {
    if !(5..=80).contains(&rows) || !(20..=240).contains(&columns) {
        Err(CodecError::InvalidDimensions)
    } else {
        Ok(())
    }
}

fn validate_counts(arguments: usize, environment: usize) -> Result<(), CodecError> {
    if arguments > MAX_ARGUMENT_COUNT {
        return Err(CodecError::TooManyArguments);
    }
    if environment > MAX_ENVIRONMENT_COUNT {
        return Err(CodecError::TooManyEnvironmentEntries);
    }
    Ok(())
}

fn validate_path(path: &[u8]) -> Result<(), CodecError> {
    validate_path_length(path.len())?;
    validate_no_nul(path)
}

fn validate_path_length(length: usize) -> Result<(), CodecError> {
    if length == 0 || length > MAX_PATH_BYTES {
        Err(CodecError::InvalidPathLength)
    } else {
        Ok(())
    }
}

fn validate_executable(executable: &[u8]) -> Result<(), CodecError> {
    if executable.first() == Some(&b'/') || !executable.contains(&b'/') {
        Ok(())
    } else {
        Err(CodecError::InvalidExecutable)
    }
}

fn validate_client_cwd(path: &[u8]) -> Result<(), CodecError> {
    if path.first() == Some(&b'/') {
        Ok(())
    } else {
        Err(CodecError::ClientWorkingDirectoryNotAbsolute)
    }
}

fn validate_no_nul(bytes: &[u8]) -> Result<(), CodecError> {
    if bytes.contains(&0) {
        Err(CodecError::NulByte)
    } else {
        Ok(())
    }
}

fn validate_environment_entry(name: &[u8], value: &[u8]) -> Result<(), CodecError> {
    if name.is_empty() || name.contains(&b'=') {
        return Err(CodecError::InvalidEnvironmentName);
    }
    validate_no_nul(name)?;
    validate_no_nul(value)?;
    if name.starts_with(RESERVED_ENVIRONMENT_PREFIX) {
        return Err(CodecError::ReservedEnvironmentName);
    }
    if name.len() > usize::from(u16::MAX) || value.len() > u32::MAX as usize {
        return Err(CodecError::LengthOverflow);
    }
    Ok(())
}

fn validate_total_len(length: usize) -> Result<(), CodecError> {
    if length > MAX_LAUNCH_REQUEST_BYTES {
        Err(CodecError::PayloadTooLarge)
    } else {
        Ok(())
    }
}

fn validate_exact_size(bytes: &[u8], expected: usize) -> Result<(), CodecError> {
    if bytes.len() < expected {
        Err(CodecError::Truncated)
    } else if bytes.len() > expected {
        Err(CodecError::TrailingBytes)
    } else {
        Ok(())
    }
}

fn validate_header(bytes: &[u8], expected_kind: u16) -> Result<(), CodecError> {
    if read_u16(bytes, 0)? != MANAGED_LAUNCH_SCHEMA {
        return Err(CodecError::UnsupportedSchema);
    }
    if read_u16(bytes, 2)? != expected_kind {
        return Err(CodecError::UnknownKind);
    }
    Ok(())
}

fn validate_unique_encoded_environment(
    encoded_environment: &[u8],
    environment_count: u16,
) -> Result<(), CodecError> {
    // The complete message and count were validated before this bounded set is
    // allocated. It stores at most one borrowed slice for each allowed entry.
    let mut names = BTreeSet::new();
    let mut cursor = Cursor::new(encoded_environment);
    for _ in 0..environment_count {
        let name_length = usize::from(cursor.read_u16()?);
        let value_length = cursor.read_u32_as_usize()?;
        let name = cursor.take(name_length)?;
        if !names.insert(name) {
            return Err(CodecError::DuplicateEnvironmentName);
        }
        cursor.take(value_length)?;
    }
    Ok(())
}

fn checked_add(left: usize, right: usize) -> Result<usize, CodecError> {
    let result = left.checked_add(right).ok_or(CodecError::LengthOverflow)?;
    validate_total_len(result)?;
    Ok(result)
}

fn outcome_code(outcome: Outcome) -> Result<u16, CodecError> {
    match outcome {
        Outcome::Accepted => Ok(1),
        Outcome::Applied => Ok(2),
        Outcome::Failed => Ok(3),
        Outcome::Unknown => Ok(4),
        Outcome::Partial => Err(CodecError::UnknownOutcome),
    }
}

fn decode_outcome(code: u16) -> Result<Outcome, CodecError> {
    match code {
        1 => Ok(Outcome::Accepted),
        2 => Ok(Outcome::Applied),
        3 => Ok(Outcome::Failed),
        4 => Ok(Outcome::Unknown),
        _ => Err(CodecError::UnknownOutcome),
    }
}

fn read_key(bytes: &[u8], offset: usize) -> Result<OperationKey, CodecError> {
    Ok(OperationKey {
        generation: read_u64(bytes, offset)?,
        lease: read_u64(bytes, offset + 8)?,
        sequence: read_u64(bytes, offset + 16)?,
    })
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, CodecError> {
    let end = offset.checked_add(2).ok_or(CodecError::LengthOverflow)?;
    let field = bytes.get(offset..end).ok_or(CodecError::Truncated)?;
    Ok(u16::from_be_bytes([field[0], field[1]]))
}

fn read_u32_as_usize(bytes: &[u8], offset: usize) -> Result<usize, CodecError> {
    usize::try_from(read_u32(bytes, offset)?).map_err(|_| CodecError::LengthOverflow)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, CodecError> {
    let end = offset.checked_add(4).ok_or(CodecError::LengthOverflow)?;
    let field = bytes.get(offset..end).ok_or(CodecError::Truncated)?;
    Ok(u32::from_be_bytes([field[0], field[1], field[2], field[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, CodecError> {
    let end = offset.checked_add(8).ok_or(CodecError::LengthOverflow)?;
    let field = bytes.get(offset..end).ok_or(CodecError::Truncated)?;
    Ok(u64::from_be_bytes([
        field[0], field[1], field[2], field[3], field[4], field[5], field[6], field[7],
    ]))
}

fn write_key<W: Write>(writer: &mut W, key: OperationKey) -> Result<(), EncodeError> {
    write_u64(writer, key.generation)?;
    write_u64(writer, key.lease)?;
    write_u64(writer, key.sequence)
}

fn write_target<W: Write>(writer: &mut W, target: TargetRef) -> Result<(), EncodeError> {
    write_u64(writer, target.generation)?;
    write_u64(writer, target.workspace)?;
    write_u64(writer, target.pane)?;
    write_u64(writer, target.session)?;
    write_u64(writer, target.occupant_revision)
}

fn write_u16<W: Write>(writer: &mut W, value: u16) -> Result<(), EncodeError> {
    writer
        .write_all(&value.to_be_bytes())
        .map_err(EncodeError::Writer)
}

fn write_u32<W: Write>(writer: &mut W, value: u32) -> Result<(), EncodeError> {
    writer
        .write_all(&value.to_be_bytes())
        .map_err(EncodeError::Writer)
}

fn write_u64<W: Write>(writer: &mut W, value: u64) -> Result<(), EncodeError> {
    writer
        .write_all(&value.to_be_bytes())
        .map_err(EncodeError::Writer)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    const fn with_offset(bytes: &'a [u8], offset: usize) -> Self {
        Self { bytes, offset }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], CodecError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(CodecError::LengthOverflow)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(CodecError::Truncated)?;
        self.offset = end;
        Ok(value)
    }

    fn read_u16(&mut self) -> Result<u16, CodecError> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn read_u32_as_usize(&mut self) -> Result<usize, CodecError> {
        usize::try_from(self.read_u32()?).map_err(|_| CodecError::LengthOverflow)
    }

    fn read_u32(&mut self) -> Result<u32, CodecError> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    const TEST_KEY: OperationKey = OperationKey {
        generation: 1,
        lease: 2,
        sequence: 3,
    };

    const TEST_TARGET: TargetRef = TargetRef {
        generation: 1,
        workspace: 4,
        pane: 5,
        session: 6,
        occupant_revision: 7,
    };

    static SAMPLE_ARGUMENTS: [&[u8]; 2] = [b"", b"\xffarg"];
    static SAMPLE_ENVIRONMENT: [EnvironmentEntry<'static>; 2] = [
        EnvironmentEntry {
            name: b"FIRST",
            value: b"",
        },
        EnvironmentEntry {
            name: b"SECOND",
            value: b"\xfevalue",
        },
    ];

    fn sample_request() -> LaunchRequest<'static> {
        LaunchRequest {
            key: TEST_KEY,
            rows: 24,
            columns: 80,
            requested_executable: b"/bin/\xfftool",
            requested_working_directory: b"relative/\xfe-dir",
            client_working_directory: b"/tmp/client/\xff-dir",
            arguments: &SAMPLE_ARGUMENTS,
            environment: &SAMPLE_ENVIRONMENT,
        }
    }

    fn make_request<'a>(
        executable: &'a [u8],
        requested_cwd: &'a [u8],
        client_cwd: &'a [u8],
        arguments: &'a [&'a [u8]],
        environment: &'a [EnvironmentEntry<'a>],
    ) -> LaunchRequest<'a> {
        LaunchRequest {
            key: TEST_KEY,
            rows: 24,
            columns: 80,
            requested_executable: executable,
            requested_working_directory: requested_cwd,
            client_working_directory: client_cwd,
            arguments,
            environment,
        }
    }

    fn encode_request(request: &LaunchRequest<'_>) -> Vec<u8> {
        let mut bytes = Vec::new();
        let written = encode_launch_request(request, &mut bytes).expect("request encodes");
        assert_eq!(written, bytes.len());
        bytes
    }

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
    }

    #[derive(Default)]
    struct RecordingWriter {
        bytes: Vec<u8>,
        calls: usize,
    }

    impl Write for RecordingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "writer-private-marker",
            ))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn launch_round_trip_borrows_non_utf8_bytes_and_preserves_order() {
        let request = sample_request();
        assert_eq!(request.operation_class(), OperationClass::DurableMutation);
        let bytes = encode_request(&request);
        assert_eq!(bytes.len(), request.encoded_len().expect("valid length"));
        assert_eq!(&bytes[..4], &[0, 6, 0, 1]);
        assert_eq!(
            &bytes[4..28],
            &[0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 3]
        );
        assert!(bytes[48..64].iter().all(|byte| *byte == 0));

        let decoded = decode_launch_request(&bytes).expect("encoded request decodes");
        assert_eq!(decoded.key(), request.key);
        assert_eq!(decoded.dimensions(), (24, 80));
        assert_eq!(decoded.requested_executable(), request.requested_executable);
        assert_eq!(
            decoded.requested_working_directory(),
            request.requested_working_directory
        );
        assert_eq!(
            decoded.client_working_directory(),
            request.client_working_directory
        );
        assert_eq!(decoded.encoded_len(), bytes.len());
        assert_eq!(
            decoded.arguments().collect::<Vec<_>>(),
            request.arguments.to_vec()
        );
        assert_eq!(
            decoded.environment().collect::<Vec<_>>(),
            request.environment.to_vec()
        );
    }

    #[test]
    fn launch_key_dimension_and_path_boundaries_are_enforced() {
        let mut request = sample_request();
        request.key = OperationKey {
            generation: u64::MAX,
            lease: u64::MAX,
            sequence: u64::MAX,
        };
        request.rows = 5;
        request.columns = 20;
        assert!(request.encoded_len().is_ok());
        request.rows = 80;
        request.columns = 240;
        assert!(request.encoded_len().is_ok());

        for key in [
            OperationKey {
                generation: 0,
                lease: 2,
                sequence: 3,
            },
            OperationKey {
                generation: 1,
                lease: 0,
                sequence: 3,
            },
            OperationKey {
                generation: 1,
                lease: 2,
                sequence: 0,
            },
        ] {
            request.key = key;
            assert_eq!(request.encoded_len(), Err(CodecError::ZeroKeyComponent));
        }

        request = sample_request();
        for rows in [4, 81] {
            request.rows = rows;
            assert_eq!(request.encoded_len(), Err(CodecError::InvalidDimensions));
        }
        request = sample_request();
        for columns in [19, 241] {
            request.columns = columns;
            assert_eq!(request.encoded_len(), Err(CodecError::InvalidDimensions));
        }
        request = sample_request();

        let mut executable = vec![b'x'; MAX_PATH_BYTES];
        executable[0] = b'/';
        let requested_cwd = vec![b'r'; MAX_PATH_BYTES];
        let mut client_cwd = vec![b'c'; MAX_PATH_BYTES];
        client_cwd[0] = b'/';
        request.requested_executable = &executable;
        request.requested_working_directory = &requested_cwd;
        request.client_working_directory = &client_cwd;
        assert!(request.encoded_len().is_ok());

        let mut overlong_executable = executable.clone();
        overlong_executable.push(b'x');
        request.requested_executable = &overlong_executable;
        assert_eq!(request.encoded_len(), Err(CodecError::InvalidPathLength));

        let overlong_requested_cwd = vec![b'r'; MAX_PATH_BYTES + 1];
        request = sample_request();
        request.requested_working_directory = &overlong_requested_cwd;
        assert_eq!(request.encoded_len(), Err(CodecError::InvalidPathLength));

        let mut overlong_client_cwd = vec![b'c'; MAX_PATH_BYTES + 1];
        overlong_client_cwd[0] = b'/';
        request = sample_request();
        request.client_working_directory = &overlong_client_cwd;
        assert_eq!(request.encoded_len(), Err(CodecError::InvalidPathLength));

        request = sample_request();
        request.requested_working_directory = b"";
        assert_eq!(request.encoded_len(), Err(CodecError::InvalidPathLength));
        request = sample_request();
        request.requested_executable = b"relative/path";
        assert_eq!(request.encoded_len(), Err(CodecError::InvalidExecutable));
        request = sample_request();
        request.client_working_directory = b"relative";
        assert_eq!(
            request.encoded_len(),
            Err(CodecError::ClientWorkingDirectoryNotAbsolute)
        );
    }

    #[test]
    fn argument_and_environment_count_limits_are_checked_before_views() {
        let arguments = vec![b"".as_slice(); MAX_ARGUMENT_COUNT];
        let request = make_request(b"tool", b".", b"/client", &arguments, &[]);
        let bytes = encode_request(&request);
        let decoded = decode_launch_request(&bytes).expect("maximum argument count decodes");
        assert_eq!(decoded.arguments().len(), MAX_ARGUMENT_COUNT);

        let too_many_arguments = vec![b"".as_slice(); MAX_ARGUMENT_COUNT + 1];
        let request = make_request(b"tool", b".", b"/client", &too_many_arguments, &[]);
        assert_eq!(request.encoded_len(), Err(CodecError::TooManyArguments));

        let names = (0..MAX_ENVIRONMENT_COUNT)
            .map(|index| format!("N{index:04}").into_bytes())
            .collect::<Vec<_>>();
        let environment = names
            .iter()
            .map(|name| EnvironmentEntry { name, value: b"" })
            .collect::<Vec<_>>();
        let request = make_request(b"tool", b".", b"/client", &[], &environment);
        let bytes = encode_request(&request);
        let decoded = decode_launch_request(&bytes).expect("maximum environment count decodes");
        assert_eq!(decoded.environment().len(), MAX_ENVIRONMENT_COUNT);
        assert_eq!(decoded.environment().next().unwrap().name, b"N0000");
        assert_eq!(decoded.environment().last().unwrap().name, b"N4095");

        let too_many_environment = vec![
            EnvironmentEntry {
                name: b"N",
                value: b"",
            };
            MAX_ENVIRONMENT_COUNT + 1
        ];
        let request = make_request(b"tool", b".", b"/client", &[], &too_many_environment);
        assert_eq!(
            request.encoded_len(),
            Err(CodecError::TooManyEnvironmentEntries)
        );

        let mut impossible = vec![0_u8; REQUEST_PREFIX_BYTES];
        put_u16(&mut impossible, 0, MANAGED_LAUNCH_SCHEMA);
        put_u16(&mut impossible, 2, KIND_LAUNCH);
        put_u64(&mut impossible, 4, 1);
        put_u64(&mut impossible, 12, 1);
        put_u64(&mut impossible, 20, 1);
        put_u16(&mut impossible, 28, 5);
        put_u16(&mut impossible, 30, 20);
        put_u16(&mut impossible, 32, MAX_ARGUMENT_COUNT as u16);
        put_u16(&mut impossible, 34, MAX_ENVIRONMENT_COUNT as u16);
        put_u32(&mut impossible, 36, 1);
        put_u32(&mut impossible, 40, 1);
        put_u32(&mut impossible, 44, 1);
        assert_eq!(
            decode_launch_request(&impossible),
            Err(CodecError::Truncated)
        );
    }

    #[test]
    fn complete_request_limit_includes_all_prefixes_and_values() {
        let argument_length = MAX_LAUNCH_REQUEST_BYTES - REQUEST_PREFIX_BYTES - 5 - 4;
        let argument = vec![b'a'; argument_length];
        let arguments = [argument.as_slice()];
        let request = make_request(b"/x", b"r", b"/c", &arguments, &[]);
        assert_eq!(request.encoded_len(), Ok(MAX_LAUNCH_REQUEST_BYTES));
        let bytes = encode_request(&request);
        assert_eq!(bytes.len(), MAX_LAUNCH_REQUEST_BYTES);
        assert_eq!(
            decode_launch_request(&bytes).unwrap().encoded_len(),
            bytes.len()
        );

        let over_limit_argument = vec![b'a'; argument_length + 1];
        let over_limit_arguments = [over_limit_argument.as_slice()];
        let over_limit = make_request(b"/x", b"r", b"/c", &over_limit_arguments, &[]);
        assert_eq!(over_limit.encoded_len(), Err(CodecError::PayloadTooLarge));
        let mut writer = RecordingWriter::default();
        assert!(matches!(
            encode_launch_request(&over_limit, &mut writer),
            Err(EncodeError::Invalid(CodecError::PayloadTooLarge))
        ));
        assert!(writer.bytes.is_empty());
        let mut one_over = bytes;
        one_over.push(0);
        assert_eq!(
            decode_launch_request(&one_over),
            Err(CodecError::PayloadTooLarge)
        );
    }

    #[test]
    fn every_nul_position_is_rejected_in_each_byte_field() {
        fn assert_invalid(
            executable: &[u8],
            requested_cwd: &[u8],
            client_cwd: &[u8],
            argument: &[u8],
            name: &[u8],
            value: &[u8],
        ) {
            let arguments = [argument];
            let environment = [EnvironmentEntry { name, value }];
            let request = make_request(
                executable,
                requested_cwd,
                client_cwd,
                &arguments,
                &environment,
            );
            assert_eq!(request.encoded_len(), Err(CodecError::NulByte));
        }

        let executable = b"/exec";
        let requested_cwd = b"cwd";
        let client_cwd = b"/client";
        let argument = b"args";
        let name = b"NAME";
        let value = b"value";
        for field in 0..6 {
            let source = match field {
                0 => executable.as_slice(),
                1 => requested_cwd.as_slice(),
                2 => client_cwd.as_slice(),
                3 => argument.as_slice(),
                4 => name.as_slice(),
                _ => value.as_slice(),
            };
            for position in 0..source.len() {
                let mut changed = source.to_vec();
                changed[position] = 0;
                let selected = changed.as_slice();
                let (exe, cwd, client, arg, env_name, env_value) = match field {
                    0 => (
                        selected,
                        requested_cwd.as_slice(),
                        client_cwd.as_slice(),
                        argument.as_slice(),
                        name.as_slice(),
                        value.as_slice(),
                    ),
                    1 => (
                        executable.as_slice(),
                        selected,
                        client_cwd.as_slice(),
                        argument.as_slice(),
                        name.as_slice(),
                        value.as_slice(),
                    ),
                    2 => (
                        executable.as_slice(),
                        requested_cwd.as_slice(),
                        selected,
                        argument.as_slice(),
                        name.as_slice(),
                        value.as_slice(),
                    ),
                    3 => (
                        executable.as_slice(),
                        requested_cwd.as_slice(),
                        client_cwd.as_slice(),
                        selected,
                        name.as_slice(),
                        value.as_slice(),
                    ),
                    4 => (
                        executable.as_slice(),
                        requested_cwd.as_slice(),
                        client_cwd.as_slice(),
                        argument.as_slice(),
                        selected,
                        value.as_slice(),
                    ),
                    _ => (
                        executable.as_slice(),
                        requested_cwd.as_slice(),
                        client_cwd.as_slice(),
                        argument.as_slice(),
                        name.as_slice(),
                        selected,
                    ),
                };
                assert_invalid(exe, cwd, client, arg, env_name, env_value);
            }
        }

        let arguments = [argument.as_slice()];
        let environment = [EnvironmentEntry { name, value }];
        let mut bytes = encode_request(&make_request(
            executable,
            requested_cwd,
            client_cwd,
            &arguments,
            &environment,
        ));
        let mut ranges = Vec::new();
        let mut offset = REQUEST_PREFIX_BYTES;
        ranges.push((offset, executable.len()));
        offset += executable.len();
        ranges.push((offset, requested_cwd.len()));
        offset += requested_cwd.len();
        ranges.push((offset, client_cwd.len()));
        offset += client_cwd.len();
        offset += 4;
        ranges.push((offset, argument.len()));
        offset += argument.len();
        offset += 6;
        ranges.push((offset, name.len()));
        offset += name.len();
        ranges.push((offset, value.len()));
        for (start, length) in ranges {
            for position in 0..length {
                let mut changed = bytes.clone();
                changed[start + position] = 0;
                assert_eq!(decode_launch_request(&changed), Err(CodecError::NulByte));
            }
        }
        bytes.clear();
    }

    #[test]
    fn environment_names_are_unique_reserved_names_rejected_and_order_retained() {
        for invalid_name in [b"".as_slice(), b"HAS=EQUALS", b"PANELINK_CHILD"] {
            let environment = [EnvironmentEntry {
                name: invalid_name,
                value: b"secret",
            }];
            let request = make_request(b"tool", b".", b"/client", &[], &environment);
            let mut writer = RecordingWriter::default();
            assert!(matches!(
                encode_launch_request(&request, &mut writer),
                Err(EncodeError::Invalid(_))
            ));
            assert_eq!(writer.calls, 0);
            assert!(writer.bytes.is_empty());
        }

        let duplicate_request = make_request(
            b"tool",
            b".",
            b"/client",
            &[],
            &[
                EnvironmentEntry {
                    name: b"DUPLICATE",
                    value: b"one",
                },
                EnvironmentEntry {
                    name: b"DUPLICATE",
                    value: b"two",
                },
            ],
        );
        assert_eq!(
            duplicate_request.encoded_len(),
            Err(CodecError::DuplicateEnvironmentName)
        );

        let mut duplicate = encode_request(&make_request(
            b"tool",
            b".",
            b"/client",
            &[],
            &[
                EnvironmentEntry {
                    name: b"ALPHA",
                    value: b"one",
                },
                EnvironmentEntry {
                    name: b"BRAVO",
                    value: b"two",
                },
            ],
        ));
        let second_name = REQUEST_PREFIX_BYTES + 4 + 1 + 7 + 6 + b"ALPHA".len() + 3 + 6;
        duplicate[second_name..second_name + 5].copy_from_slice(b"ALPHA");
        assert_eq!(
            decode_launch_request(&duplicate),
            Err(CodecError::DuplicateEnvironmentName)
        );

        let mut reserved = encode_request(&make_request(
            b"tool",
            b".",
            b"/client",
            &[],
            &[EnvironmentEntry {
                name: b"CUSTOMNAME",
                value: b"",
            }],
        ));
        let name_start = REQUEST_PREFIX_BYTES + 4 + 1 + 7 + 6;
        reserved[name_start..name_start + 10].copy_from_slice(b"PANELINK_X");
        assert_eq!(
            decode_launch_request(&reserved),
            Err(CodecError::ReservedEnvironmentName)
        );

        let mut equals = encode_request(&make_request(
            b"tool",
            b".",
            b"/client",
            &[],
            &[EnvironmentEntry {
                name: b"NAME_X",
                value: b"",
            }],
        ));
        let name_start = REQUEST_PREFIX_BYTES + 4 + 1 + 7 + 6;
        equals[name_start + 4] = b'=';
        assert_eq!(
            decode_launch_request(&equals),
            Err(CodecError::InvalidEnvironmentName)
        );

        let mut empty_name = encode_request(&make_request(
            b"tool",
            b".",
            b"/client",
            &[],
            &[EnvironmentEntry {
                name: b"NAME",
                value: b"value",
            }],
        ));
        let entry_start = REQUEST_PREFIX_BYTES + 4 + 1 + 7;
        put_u16(&mut empty_name, entry_start, 0);
        empty_name.drain(entry_start + 6..entry_start + 10);
        assert_eq!(
            decode_launch_request(&empty_name),
            Err(CodecError::InvalidEnvironmentName)
        );

        let ordered = make_request(
            b"tool",
            b".",
            b"/client",
            &[],
            &[
                EnvironmentEntry {
                    name: b"Z_LAST",
                    value: b"last",
                },
                EnvironmentEntry {
                    name: b"A_FIRST",
                    value: b"first",
                },
            ],
        );
        let ordered_bytes = encode_request(&ordered);
        let decoded = decode_launch_request(&ordered_bytes).unwrap();
        assert_eq!(
            decoded
                .environment()
                .map(|entry| entry.name)
                .collect::<Vec<_>>(),
            vec![b"Z_LAST".as_slice(), b"A_FIRST".as_slice()]
        );
    }

    #[test]
    fn launch_decoder_rejects_reserved_unknown_truncated_and_trailing_bytes() {
        let original = encode_request(&sample_request());
        for end in 0..original.len() {
            assert_eq!(
                decode_launch_request(&original[..end]),
                Err(CodecError::Truncated),
                "truncation at {end}"
            );
        }
        let mut trailing = original.clone();
        trailing.push(1);
        assert_eq!(
            decode_launch_request(&trailing),
            Err(CodecError::TrailingBytes)
        );

        let mut unknown_schema = original.clone();
        put_u16(&mut unknown_schema, 0, MANAGED_LAUNCH_SCHEMA + 1);
        assert_eq!(
            decode_launch_request(&unknown_schema),
            Err(CodecError::UnsupportedSchema)
        );
        let mut unknown_kind = original.clone();
        put_u16(&mut unknown_kind, 2, 99);
        assert_eq!(
            decode_launch_request(&unknown_kind),
            Err(CodecError::UnknownKind)
        );

        for offset in 48..64 {
            let mut reserved = original.clone();
            reserved[offset] = 1;
            assert_eq!(
                decode_launch_request(&reserved),
                Err(CodecError::NonzeroReserved)
            );
        }
        for offset in [4, 12, 20] {
            let mut zero_key = original.clone();
            put_u64(&mut zero_key, offset, 0);
            assert_eq!(
                decode_launch_request(&zero_key),
                Err(CodecError::ZeroKeyComponent)
            );
        }
        for (offset, value) in [(28, 4), (28, 81), (30, 19), (30, 241)] {
            let mut bad_dimension = original.clone();
            put_u16(&mut bad_dimension, offset, value);
            assert_eq!(
                decode_launch_request(&bad_dimension),
                Err(CodecError::InvalidDimensions)
            );
        }
        let mut too_many_arguments = original.clone();
        put_u16(&mut too_many_arguments, 32, (MAX_ARGUMENT_COUNT + 1) as u16);
        assert_eq!(
            decode_launch_request(&too_many_arguments),
            Err(CodecError::TooManyArguments)
        );
        let mut too_many_environment = original.clone();
        put_u16(
            &mut too_many_environment,
            34,
            (MAX_ENVIRONMENT_COUNT + 1) as u16,
        );
        assert_eq!(
            decode_launch_request(&too_many_environment),
            Err(CodecError::TooManyEnvironmentEntries)
        );

        let mut path_too_long = original.clone();
        put_u32(&mut path_too_long, 36, (MAX_PATH_BYTES + 1) as u32);
        assert_eq!(
            decode_launch_request(&path_too_long),
            Err(CodecError::InvalidPathLength)
        );

        let mut huge_argument =
            encode_request(&make_request(b"tool", b".", b"/client", &[b"a"], &[]));
        let arg_prefix = REQUEST_PREFIX_BYTES + b"tool".len() + 1 + b"/client".len();
        put_u32(&mut huge_argument, arg_prefix, u32::MAX);
        assert_eq!(
            decode_launch_request(&huge_argument),
            Err(CodecError::Truncated)
        );

        let mut huge_value = encode_request(&make_request(
            b"tool",
            b".",
            b"/client",
            &[],
            &[EnvironmentEntry {
                name: b"N",
                value: b"V",
            }],
        ));
        let env_entry = REQUEST_PREFIX_BYTES + b"tool".len() + 1 + b"/client".len();
        put_u32(&mut huge_value, env_entry + 2, u32::MAX);
        assert_eq!(
            decode_launch_request(&huge_value),
            Err(CodecError::Truncated)
        );
    }

    #[test]
    fn status_query_round_trip_is_exact_and_redacted() {
        let query = LaunchStatusQuery {
            key: OperationKey {
                generation: u64::MAX,
                lease: u64::MAX - 1,
                sequence: u64::MAX - 2,
            },
        };
        assert_eq!(query.operation_class(), OperationClass::Query);
        assert_eq!(query.encoded_len(), Ok(STATUS_QUERY_BYTES));
        let mut bytes = Vec::new();
        assert_eq!(
            encode_status_query(query, &mut bytes).unwrap(),
            STATUS_QUERY_BYTES
        );
        assert_eq!(&bytes[..4], &[0, 6, 0, 2]);
        assert_eq!(decode_status_query(&bytes), Ok(query));
        assert_eq!(
            decode_status_query(&bytes[..31]),
            Err(CodecError::Truncated)
        );
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            decode_status_query(&trailing),
            Err(CodecError::TrailingBytes)
        );
        for offset in [0, 2] {
            let mut unknown = bytes.clone();
            put_u16(&mut unknown, offset, 99);
            let expected = if offset == 0 {
                CodecError::UnsupportedSchema
            } else {
                CodecError::UnknownKind
            };
            assert_eq!(decode_status_query(&unknown), Err(expected));
        }
        for offset in 28..32 {
            let mut reserved = bytes.clone();
            reserved[offset] = 1;
            assert_eq!(
                decode_status_query(&reserved),
                Err(CodecError::NonzeroReserved)
            );
        }
        for offset in [4, 12, 20] {
            let mut zero_key = bytes.clone();
            put_u64(&mut zero_key, offset, 0);
            assert_eq!(
                decode_status_query(&zero_key),
                Err(CodecError::ZeroKeyComponent)
            );
        }
        assert!(!format!("{query:?}").contains(&query.key.generation.to_string()));
    }

    #[test]
    fn result_codes_target_contract_and_reserved_bytes_are_enforced() {
        let valid = [
            (Outcome::Accepted, Some(TEST_TARGET)),
            (Outcome::Applied, Some(TEST_TARGET)),
            (Outcome::Failed, None),
            (Outcome::Failed, Some(TEST_TARGET)),
            (Outcome::Unknown, Some(TEST_TARGET)),
        ];
        for (outcome, target) in valid {
            let result = LaunchResult {
                key: TEST_KEY,
                outcome,
                target,
            };
            assert_eq!(result.encoded_len(), Ok(RESULT_BYTES));
            let mut bytes = Vec::new();
            assert_eq!(
                encode_launch_result(&result, &mut bytes).unwrap(),
                RESULT_BYTES
            );
            assert_eq!(decode_launch_result(&bytes), Ok(result));
            let expected_code = match outcome {
                Outcome::Accepted => 1,
                Outcome::Applied => 2,
                Outcome::Failed => 3,
                Outcome::Unknown => 4,
                Outcome::Partial => unreachable!(),
            };
            assert_eq!(u16::from_be_bytes([bytes[28], bytes[29]]), expected_code);
            if target.is_none() {
                assert!(bytes[32..72].iter().all(|byte| *byte == 0));
            }
        }

        let applied = LaunchResult {
            key: TEST_KEY,
            outcome: Outcome::Applied,
            target: Some(TEST_TARGET),
        };
        let original = {
            let mut bytes = Vec::new();
            encode_launch_result(&applied, &mut bytes).unwrap();
            bytes
        };
        assert_eq!(
            decode_launch_result(&original[..79]),
            Err(CodecError::Truncated)
        );
        let mut trailing = original.clone();
        trailing.push(0);
        assert_eq!(
            decode_launch_result(&trailing),
            Err(CodecError::TrailingBytes)
        );
        let mut unknown_schema = original.clone();
        put_u16(&mut unknown_schema, 0, 7);
        assert_eq!(
            decode_launch_result(&unknown_schema),
            Err(CodecError::UnsupportedSchema)
        );
        let mut unknown_kind = original.clone();
        put_u16(&mut unknown_kind, 2, 7);
        assert_eq!(
            decode_launch_result(&unknown_kind),
            Err(CodecError::UnknownKind)
        );
        for offset in 72..80 {
            let mut reserved = original.clone();
            reserved[offset] = 1;
            assert_eq!(
                decode_launch_result(&reserved),
                Err(CodecError::NonzeroReserved)
            );
        }
        let mut unknown_outcome = original.clone();
        put_u16(&mut unknown_outcome, 28, 5);
        assert_eq!(
            decode_launch_result(&unknown_outcome),
            Err(CodecError::UnknownOutcome)
        );
        let mut invalid_flag = original.clone();
        put_u16(&mut invalid_flag, 30, 2);
        assert_eq!(
            decode_launch_result(&invalid_flag),
            Err(CodecError::InvalidTargetPresence)
        );
        let mut no_target_nonzero = original.clone();
        put_u16(&mut no_target_nonzero, 30, 0);
        assert_eq!(
            decode_launch_result(&no_target_nonzero),
            Err(CodecError::NonzeroReserved)
        );
        let mut mismatch_generation = original.clone();
        put_u64(&mut mismatch_generation, 32, 2);
        assert_eq!(
            decode_launch_result(&mismatch_generation),
            Err(CodecError::InvalidTarget)
        );
        for offset in [32, 40, 48, 56, 64] {
            let mut zero_target = original.clone();
            put_u64(&mut zero_target, offset, 0);
            assert_eq!(
                decode_launch_result(&zero_target),
                Err(CodecError::InvalidTarget)
            );
        }

        for result in [
            LaunchResult {
                outcome: Outcome::Accepted,
                target: None,
                ..applied
            },
            LaunchResult {
                outcome: Outcome::Applied,
                target: None,
                ..applied
            },
            LaunchResult {
                outcome: Outcome::Unknown,
                target: None,
                ..applied
            },
            LaunchResult {
                outcome: Outcome::Partial,
                target: None,
                ..applied
            },
        ] {
            let expected = if result.outcome == Outcome::Partial {
                CodecError::UnknownOutcome
            } else {
                CodecError::OutcomeTargetMismatch
            };
            assert_eq!(result.encoded_len(), Err(expected));
            // Exercise the decoder independently of the encoder: construct a
            // valid envelope, then remove the target required by the contract.
            let mut wire = original.clone();
            if result.outcome != Outcome::Partial {
                put_u16(&mut wire, 28, outcome_code(result.outcome).unwrap());
                put_u16(&mut wire, 30, 0);
                wire[32..72].fill(0);
                assert_eq!(decode_launch_result(&wire), Err(expected));
                let mut writer = RecordingWriter::default();
                assert!(matches!(
                    encode_launch_result(&result, &mut writer),
                    Err(EncodeError::Invalid(CodecError::OutcomeTargetMismatch))
                ));
                assert_eq!(writer.calls, 0);
            }
        }
        for offset in [4, 12, 20] {
            let mut zero_key = original.clone();
            put_u64(&mut zero_key, offset, 0);
            assert_eq!(
                decode_launch_result(&zero_key),
                Err(CodecError::ZeroKeyComponent)
            );
        }
    }

    #[test]
    fn invalid_values_write_nothing_and_writer_errors_are_preserved_redacted() {
        let mut invalid_request = sample_request();
        invalid_request.key.lease = 0;
        let mut writer = RecordingWriter::default();
        assert!(matches!(
            encode_launch_request(&invalid_request, &mut writer),
            Err(EncodeError::Invalid(CodecError::ZeroKeyComponent))
        ));
        assert_eq!(writer.calls, 0);
        assert!(writer.bytes.is_empty());

        let invalid_result = LaunchResult {
            key: TEST_KEY,
            outcome: Outcome::Applied,
            target: None,
        };
        assert!(matches!(
            encode_launch_result(&invalid_result, &mut writer),
            Err(EncodeError::Invalid(CodecError::OutcomeTargetMismatch))
        ));
        assert_eq!(writer.calls, 0);

        let valid = sample_request();
        let error = encode_launch_request(&valid, &mut FailingWriter).unwrap_err();
        match &error {
            EncodeError::Writer(error) => assert_eq!(error.kind(), io::ErrorKind::BrokenPipe),
            EncodeError::Invalid(_) => panic!("valid request must reach the writer"),
        }
        let error_text = format!("{error}");
        let error_debug = format!("{error:?}");
        assert!(!error_text.contains("writer-private-marker"));
        assert!(!error_debug.contains("writer-private-marker"));
    }

    #[test]
    fn debug_and_error_formatting_never_disclose_caller_fields() {
        let secret = b"/private/launch/path";
        let arguments: [&[u8]; 1] = [b"secret-argument"];
        let environment = [EnvironmentEntry {
            name: b"SECRET_NAME",
            value: b"secret-value",
        }];
        let mut request = make_request(secret, b"relative", b"/client", &arguments, &environment);
        request.key = OperationKey {
            generation: 0x1122_3344_5566_7788,
            lease: 0x8877_6655_4433_2211,
            sequence: 99,
        };
        let debug = format!("{request:?} {:?}", environment[0]);
        for private in [
            "/private/launch/path",
            "secret-argument",
            "SECRET_NAME",
            "secret-value",
            "1234605616436508552",
        ] {
            assert!(!debug.contains(private));
        }
        request.key.generation = 0;
        let error = request.encoded_len().unwrap_err();
        assert!(!error.to_string().contains("private"));
        assert!(!format!("{error:?}").contains("private"));

        let query = LaunchStatusQuery { key: request.key };
        let result = LaunchResult {
            key: request.key,
            outcome: Outcome::Unknown,
            target: None,
        };
        assert!(!format!("{query:?}").contains("1234605616436508552"));
        assert!(!format!("{result:?}").contains("1234605616436508552"));
    }
}
