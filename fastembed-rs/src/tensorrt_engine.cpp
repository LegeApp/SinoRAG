#include "tensorrt_engine.h"

#include <NvInferPlugin.h>
#include <fstream>
#include <iostream>
#include <mutex>
#include <stdexcept>

#include "fastembed/src/tensorrt.rs.h"

using namespace nvinfer1;

static Logger &getGlobalLogger() {
  static Logger instance;
  return instance;
}

void Logger::log(nvinfer1::ILogger::Severity severity,
                 const char *msg) noexcept {
  if (severity > reportableSeverity) {
    return;
  }
  std::cerr << "[fastembed:tensorrt] " << msg << std::endl;
}

static void checkCudaErrorCode(cudaError_t code) {
  if (code != cudaSuccess) {
    std::string errMsg =
        "CUDA operation failed with code: " + std::to_string(code) + " (" +
        cudaGetErrorName(code) + "): " + cudaGetErrorString(code);
    throw std::runtime_error(errMsg);
  }
}

template <typename T> static void resize(rust::Vec<T> &v, size_t len) {
  v.reserve(len);
  while (v.size() < len) {
    v.push_back(T{});
  }
}

std::unique_ptr<Engine> load_engine(const Options &options) {
  auto engine = std::make_unique<Engine>(options);
  engine->load();
  return engine;
}

Engine::Engine(const Options &options) : kEnginePath(options.path) {
  const char *rustLogLevel = getenv("RUST_LOG");
  if (rustLogLevel != nullptr) {
    std::string level(rustLogLevel);
    if (level == "error") {
      mLogger.reportableSeverity = ILogger::Severity::kERROR;
    } else if (level == "info") {
      mLogger.reportableSeverity = ILogger::Severity::kINFO;
    } else if (level == "debug" || level == "trace") {
      mLogger.reportableSeverity = ILogger::Severity::kVERBOSE;
    }
  }
}

void Engine::load() {
  const char *initPlugins = getenv("FASTEMBED_TRT_INIT_PLUGINS");
  if (initPlugins != nullptr && std::string(initPlugins) == "1") {
    if (!initLibNvInferPlugins(&getGlobalLogger(), "")) {
      throw std::runtime_error("TensorRT plugin initialization failed");
    }
  }

  std::ifstream file(kEnginePath, std::ios::binary | std::ios::ate);
  if (!file.is_open()) {
    throw std::runtime_error("Could not open TensorRT engine file: " +
                             kEnginePath);
  }

  std::streamsize size = file.tellg();
  file.seekg(0, std::ios::beg);
  if (file.fail()) {
    throw std::runtime_error("Failed to seek in TensorRT engine file");
  }

  std::vector<char> buffer(size);
  if (!file.read(buffer.data(), size)) {
    throw std::runtime_error("Unable to read TensorRT engine file");
  }

  mRuntime = std::unique_ptr<IRuntime>{createInferRuntime(getGlobalLogger())};
  if (!mRuntime) {
    throw std::runtime_error("TensorRT runtime not initialized");
  }

  mEngine = std::unique_ptr<ICudaEngine>(
      mRuntime->deserializeCudaEngine(buffer.data(), buffer.size()));
  if (!mEngine) {
    throw std::runtime_error("TensorRT engine deserialization failed");
  }

  mContext =
      std::unique_ptr<IExecutionContext>(mEngine->createExecutionContext());
  if (!mContext) {
    throw std::runtime_error("Could not create TensorRT execution context");
  }

  mTensorMetadata.clear();
  mTensorMetadata.reserve(mEngine->getNbIOTensors());

  for (int i = 0; i < mEngine->getNbIOTensors(); ++i) {
    const auto tensorName = mEngine->getIOTensorName(i);
    const auto tensorType = mEngine->getTensorIOMode(tensorName);
    const auto tensorShape = mEngine->getTensorShape(tensorName);
    const auto tensorDataType = mEngine->getTensorDataType(tensorName);

    TensorMetadata metadata;
    metadata.name = std::string(tensorName);
    metadata.ioMode = tensorType;
    metadata.dataType = tensorDataType;
    metadata.dims = tensorShape;
    mTensorMetadata.push_back(std::move(metadata));

    if (tensorType == TensorIOMode::kINPUT) {
      int32_t minBatch =
          mEngine->getProfileShape(tensorName, 0, OptProfileSelector::kMIN)
              .d[0];
      int32_t optBatch =
          mEngine->getProfileShape(tensorName, 0, OptProfileSelector::kOPT)
              .d[0];
      int32_t maxBatch =
          mEngine->getProfileShape(tensorName, 0, OptProfileSelector::kMAX)
              .d[0];

      if (mMinBatchSize == 0) {
        mMinBatchSize = minBatch;
        mOptBatchSize = optBatch;
        mMaxBatchSize = maxBatch;
      } else if (minBatch != mMinBatchSize || optBatch != mOptBatchSize ||
                 maxBatch != mMaxBatchSize) {
        throw std::runtime_error(
            "Inconsistent batch profile across TensorRT input tensors");
      }
    }
  }
}

void Engine::enqueue(const uint64_t *input_ptrs, size_t num_inputs,
                     const uint64_t *output_ptrs, size_t num_outputs,
                     cudaStream_t stream, uint32_t batch_size) {
  if (batch_size < static_cast<uint32_t>(mMinBatchSize) ||
      batch_size > static_cast<uint32_t>(mMaxBatchSize)) {
    throw std::runtime_error("Batch size outside TensorRT optimization profile");
  }

  size_t inputIdx = 0;
  size_t outputIdx = 0;

  for (const auto &meta : mTensorMetadata) {
    if (meta.ioMode == TensorIOMode::kINPUT) {
      if (inputIdx >= num_inputs) {
        throw std::runtime_error("Missing TensorRT input pointer");
      }
      Dims inputDims = meta.dims;
      inputDims.d[0] = batch_size;
      if (!mContext->setInputShape(meta.name.c_str(), inputDims)) {
        throw std::runtime_error("Failed to set TensorRT input shape for " +
                                 meta.name);
      }
      if (!mContext->setTensorAddress(
              meta.name.c_str(),
              reinterpret_cast<void *>(input_ptrs[inputIdx]))) {
        throw std::runtime_error("Failed to set TensorRT input address for " +
                                 meta.name);
      }
      inputIdx++;
    } else {
      if (outputIdx >= num_outputs) {
        throw std::runtime_error("Missing TensorRT output pointer");
      }
      if (!mContext->setTensorAddress(
              meta.name.c_str(),
              reinterpret_cast<void *>(output_ptrs[outputIdx]))) {
        throw std::runtime_error("Failed to set TensorRT output address for " +
                                 meta.name);
      }
      outputIdx++;
    }
  }

  if (!mContext->allInputDimensionsSpecified()) {
    throw std::runtime_error("Not all TensorRT input dimensions were specified");
  }

  if (!mContext->enqueueV3(stream)) {
    throw std::runtime_error("TensorRT inference execution failed");
  }
}

HostOutput Engine::infer_i32_host(rust::Slice<const int32_t> input_ids,
                                  rust::Slice<const int32_t> attention_mask,
                                  rust::Slice<const int32_t> token_type_ids,
                                  uint32_t batch_size, uint32_t seq_len) {
  const size_t token_count = static_cast<size_t>(batch_size) * seq_len;
  if (input_ids.size() != token_count ||
      attention_mask.size() != token_count) {
    throw std::runtime_error("input_ids/attention_mask length mismatch");
  }
  if (!token_type_ids.empty() && token_type_ids.size() != token_count) {
    throw std::runtime_error("token_type_ids length mismatch");
  }

  cudaStream_t stream = nullptr;
  checkCudaErrorCode(cudaStreamCreate(&stream));

  std::vector<void *> input_allocs;
  std::vector<void *> output_allocs;
  std::vector<uint64_t> input_ptrs;
  std::vector<uint64_t> output_ptrs;
  std::vector<std::vector<int64_t>> host_i64_inputs;
  rust::Vec<float> host_output;
  HostOutput result;

  try {
    for (const auto &meta : mTensorMetadata) {
      if (meta.ioMode != TensorIOMode::kINPUT) {
        continue;
      }
      if (meta.dataType != DataType::kINT32 && meta.dataType != DataType::kINT64) {
        throw std::runtime_error("TensorRT embedding inputs must be INT32 or INT64: " +
                                 meta.name);
      }
      if (meta.dims.nbDims != 2 ||
          static_cast<uint32_t>(meta.dims.d[1]) != seq_len) {
        throw std::runtime_error("TensorRT input sequence length mismatch: " +
                                 meta.name);
      }

      const int32_t *src_i32 = nullptr;
      if (meta.name == "input_ids") {
        src_i32 = input_ids.data();
      } else if (meta.name == "attention_mask") {
        src_i32 = attention_mask.data();
      } else if (meta.name == "token_type_ids") {
        if (token_type_ids.empty()) {
          throw std::runtime_error("engine requires token_type_ids");
        }
        src_i32 = token_type_ids.data();
      } else {
        throw std::runtime_error("Unsupported TensorRT embedding input: " +
                                 meta.name);
      }

      void *device = nullptr;
      const void *src = src_i32;
      size_t bytes = token_count * sizeof(int32_t);
      if (meta.dataType == DataType::kINT64) {
        host_i64_inputs.emplace_back();
        auto &converted = host_i64_inputs.back();
        converted.reserve(token_count);
        for (size_t i = 0; i < token_count; ++i) {
          converted.push_back(static_cast<int64_t>(src_i32[i]));
        }
        src = converted.data();
        bytes = token_count * sizeof(int64_t);
      }
      checkCudaErrorCode(cudaMalloc(&device, bytes));
      input_allocs.push_back(device);
      checkCudaErrorCode(
          cudaMemcpyAsync(device, src, bytes, cudaMemcpyHostToDevice, stream));
      input_ptrs.push_back(reinterpret_cast<uint64_t>(device));
    }

    size_t output_len = 0;
    size_t output_dim = 0;
    HostOutputKind output_kind = HostOutputKind::SentenceEmbedding;
    for (const auto &meta : mTensorMetadata) {
      if (meta.ioMode != TensorIOMode::kOUTPUT) {
        continue;
      }
      if (!output_allocs.empty()) {
        throw std::runtime_error("embedding runner supports exactly one output");
      }
      if (meta.dataType != DataType::kFLOAT) {
        throw std::runtime_error("embedding output must be FP32");
      }
      if (meta.dims.nbDims == 2) {
        output_kind = HostOutputKind::SentenceEmbedding;
        output_dim = static_cast<size_t>(meta.dims.d[1]);
        output_len = static_cast<size_t>(batch_size) * output_dim;
      } else if (meta.dims.nbDims == 3) {
        output_kind = HostOutputKind::LastHiddenState;
        output_dim = static_cast<size_t>(meta.dims.d[2]);
        output_len = static_cast<size_t>(batch_size) * seq_len * output_dim;
      } else {
        throw std::runtime_error("embedding output must be [B,D] or [B,S,D]");
      }

      void *device = nullptr;
      checkCudaErrorCode(cudaMalloc(&device, output_len * sizeof(float)));
      output_allocs.push_back(device);
      output_ptrs.push_back(reinterpret_cast<uint64_t>(device));
    }

    enqueue(input_ptrs.data(), input_ptrs.size(), output_ptrs.data(),
            output_ptrs.size(), stream, batch_size);

    resize(host_output, output_len);
    checkCudaErrorCode(cudaMemcpyAsync(host_output.data(), output_allocs[0],
                                       output_len * sizeof(float),
                                       cudaMemcpyDeviceToHost, stream));
    checkCudaErrorCode(cudaStreamSynchronize(stream));

    result.kind = output_kind;
    result.dim = output_dim;
    result.data = std::move(host_output);
  } catch (...) {
    for (void *ptr : input_allocs) {
      cudaFree(ptr);
    }
    for (void *ptr : output_allocs) {
      cudaFree(ptr);
    }
    cudaStreamDestroy(stream);
    throw;
  }

  for (void *ptr : input_allocs) {
    checkCudaErrorCode(cudaFree(ptr));
  }
  for (void *ptr : output_allocs) {
    checkCudaErrorCode(cudaFree(ptr));
  }
  checkCudaErrorCode(cudaStreamDestroy(stream));
  return result;
}
