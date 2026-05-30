#pragma once

#include "NvInfer.h"
#include <cuda_runtime.h>
#include <memory>
#include <string>
#include <vector>

#include "rust/cxx.h"

struct HostOutput;
struct Options;
enum class HostOutputKind : uint8_t;

class Logger : public nvinfer1::ILogger {
public:
  nvinfer1::ILogger::Severity reportableSeverity =
      nvinfer1::ILogger::Severity::kWARNING;

  void log(nvinfer1::ILogger::Severity severity,
           const char *msg) noexcept override;
};

class Engine {
public:
  explicit Engine(const Options &options);
  ~Engine() = default;

  void load();

  HostOutput infer_i32_host(rust::Slice<const int32_t> input_ids,
                            rust::Slice<const int32_t> attention_mask,
                            rust::Slice<const int32_t> token_type_ids,
                            uint32_t batch_size, uint32_t seq_len);

private:
  struct TensorMetadata {
    std::string name;
    nvinfer1::TensorIOMode ioMode;
    nvinfer1::DataType dataType;
    nvinfer1::Dims dims;
  };

  void enqueue(const uint64_t *input_ptrs, size_t num_inputs,
               const uint64_t *output_ptrs, size_t num_outputs,
               cudaStream_t stream, uint32_t batch_size);

  std::vector<TensorMetadata> mTensorMetadata;
  int32_t mMinBatchSize = 0;
  int32_t mOptBatchSize = 0;
  int32_t mMaxBatchSize = 0;

  std::unique_ptr<nvinfer1::IRuntime> mRuntime;
  std::unique_ptr<nvinfer1::ICudaEngine> mEngine;
  std::unique_ptr<nvinfer1::IExecutionContext> mContext;
  Logger mLogger;

  const std::string kEnginePath;
};

std::unique_ptr<Engine> load_engine(const Options &options);
