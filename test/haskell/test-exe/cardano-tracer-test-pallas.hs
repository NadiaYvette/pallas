{-# LANGUAGE NumericUnderscores #-}
{-# LANGUAGE RecordWildCards #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE OverloadedStrings #-}
{-# LANGUAGE LambdaCase #-}
{-# OPTIONS_GHC -Wno-unused-matches #-}

-- | Test trace-compat (Rust tracer) with EKG, DataPoints, and TraceObjects
--   file output verification.

import           Cardano.Logging
import qualified Cardano.Logging.Types as Net
import           Cardano.Tracer.Test.ForwardingStressTest.Script
import           Cardano.Tracer.Test.ForwardingStressTest.Types
import           Cardano.Tracer.Test.Utils
import           Ouroboros.Network.Magic (NetworkMagic (..))
import           Ouroboros.Network.NodeToClient (withIOManager)

import           Control.Concurrent (threadDelay)
import           Control.Exception
import           Control.Monad (when)
import           Data.IORef (IORef, newIORef, readIORef, writeIORef)
import qualified Data.List as L
import           Data.Maybe (fromMaybe)
import           Data.Monoid
import           Data.Vector (Vector)
import qualified Data.Vector as Vector
import qualified System.Directory as Sys
import           System.Environment (lookupEnv, setEnv, unsetEnv)
import qualified System.IO as Sys
import           System.PosixCompat.Files (fileExist)
import qualified System.Process as Sys

import           Test.Tasty
import           Test.Tasty.QuickCheck

import           Trace.Forward.Forwarding (initForwarding)
import           Trace.Forward.Utils.TraceObject (writeToSink)
import qualified System.Metrics as EKG
import qualified System.Metrics.Gauge as Gauge
import           Data.Aeson (decode, Value(..), Object)
import qualified Data.Aeson.KeyMap as KeyMap
import qualified Data.ByteString.Lazy as BSL
import qualified Data.ByteString.Lazy.Char8 as BS8

main :: IO ()
main = do
    setEnv "TASTY_NUM_THREADS" "1"
    -- Resolve PALLAS_BIN_DIR to absolute path before we chdir to the workdir
    binDir <- fromMaybe "../../../target/release" <$> lookupEnv "PALLAS_BIN_DIR"
    absBinDir <- Sys.makeAbsolute binDir
    setEnv "PALLAS_BIN_DIR" absBinDir
    mbWorkdir <- lookupEnv "WORKDIR"

    ts' <- getTestSetup
             TestSetup
             { tsTime         = Last $ Just 10.0
             , tsThreads      = Last $ Just 5
             , tsMessages     = Last   Nothing
             , tsSockInternal = Last $ Just "tracer.sock"
             , tsSockExternal = Last $ Just "tracer.sock"
             , tsNetworkMagic = Last $ Just $ NetworkMagic 42
             , tsWorkDir      = Last $ Just $ fromMaybe "/tmp/testTracerPallas" mbWorkdir
             }

    tracerRoot <- Sys.canonicalizePath $ unI (tsWorkDir ts')
    exists <- fileExist tracerRoot
    when exists do
      Sys.removeDirectoryRecursive           tracerRoot
    Sys.createDirectoryIfMissing True       (tracerRoot <> "/logs")
    Sys.setCurrentDirectory                  tracerRoot

    let ts = ts' { tsWorkDir = Identity tracerRoot }

    msgCountersRef <- newIORef []
    msgsRef        <- newIORef Vector.empty
    tracerRef      <- newIORef Nothing

    let tracerGetter = getPallasTracerState ts tracerRef

    defaultMain (pallasTests ts msgCountersRef msgsRef tracerGetter)
        `catch` (\ (e :: SomeException) -> do
            unsetEnv "TASTY_NUM_THREADS"
            trState <- readIORef tracerRef
            case trState of
              Nothing -> pure ()
              Just (tracerHdl, _, _, _) ->
                Sys.cleanupProcess (Nothing, Nothing, Nothing, tracerHdl)
            throwIO e)

pallasTests :: TestSetup Identity
            -> IORef [Int]
            -> IORef (Vector Message)
            -> IO (Sys.ProcessHandle, Trace IO Message, EKG.Store, DataPointStore)
            -> TestTree
pallasTests ts msgCountersRef msgsRef tracerGetter =
    testGroup "Pallas Interop Tests"
    [ localOption (QuickCheckTests 1) $ testGroup "trace-forwarder"
        [ testProperty "TraceObjects + EKG + Datapoints" $
            runPallasTest ts msgCountersRef msgsRef tracerGetter
        ]
    ]

getPallasTracerState :: TestSetup Identity
                     -> IORef (Maybe (Sys.ProcessHandle, Trace IO Message, EKG.Store, DataPointStore))
                     -> IO (Sys.ProcessHandle, Trace IO Message, EKG.Store, DataPointStore)
getPallasTracerState TestSetup{..} ref = do
  state <- readIORef ref
  case state of
    Just st -> pure st
    Nothing -> do
      stdTr <- standardTracer

      -- Create EKG Store
      ekgStore <- EKG.newStore

      (procHdl, fwdTr, dpStore) <- setupFwdTracer ekgStore

      ekgTr <- ekgTracer simpleTestConfig ekgStore

      -- Pass EKG tracer
      tr <- mkCardanoTracer stdTr fwdTr (Just ekgTr) ["Test"]
      let st = (procHdl, tr, ekgStore, dpStore)
      writeIORef ref $ Just st
      pure st
 where
   setupFwdTracer ekgStore = do
     Sys.writeFile "config.yaml" . L.unlines $
       [ "networkMagic: " <> show (unNetworkMagic $ unI tsNetworkMagic)
       , "network:"
       , "  tag: AcceptAt"
       , "  contents: \""<> unI tsSockExternal <>"\""
       , "logging:"
       , "- logRoot: \"logs\""
       , "  logMode: FileMode"
       , "  logFormat: ForMachine"
       ]
     -- Point to trace-compat
     binDir <- fromMaybe "../../../target/release" <$> lookupEnv "PALLAS_BIN_DIR"
     externalTracerHdl <- Sys.spawnProcess (binDir <> "/trace-compat")
       [ "--config" ,    "config.yaml"
       ]
     threadDelay 1_000_000
     res <- Sys.getProcessExitCode externalTracerHdl
     case res of
       Nothing   -> putStrLn "trace-compat started.."
       Just code -> error $ "trace-compat failed to start with code " <> show code

     (forwardSink, dpStore) <- withIOManager \iomgr -> do
       let forwardingConf = fromMaybe defaultForwarder (tcForwarder simpleTestConfig)
       initForwarding iomgr forwardingConf (unI tsNetworkMagic) (Just ekgStore)
         (Just (Net.LocalPipe (unI tsSockExternal), Initiator))
     pure (externalTracerHdl, forwardTracer (writeToSink forwardSink), dpStore)

runPallasTest :: TestSetup Identity
              -> IORef [Int]
              -> IORef (Vector Message)
              -> IO (Sys.ProcessHandle, Trace IO Message, EKG.Store, DataPointStore)
              -> Property
runPallasTest TestSetup{..} _msgCountersRef _msgsRef tracerGetter = ioProperty do
    (_, tr, ekgStore, dpStore) <- tracerGetter

    -- 1. EKG Test
    -- Manually update EKG
    g <- EKG.createGauge "test.gauge" ekgStore
    Gauge.set g 123

    -- 2. Datapoint Test
    writeToStore dpStore "test.datapoint" (DataPoint (String "datapoint-value"))

    -- 3. TraceObject Test
    let msg = Message1 42 100
    traceWith tr msg

    -- 4. Wait for forwarding and polling
    threadDelay 5_000_000

    -- 5. Verify
    let logsDir = unI tsWorkDir <> "/logs/sock@0"

    -- Check EKG
    ekgContent <- BSL.readFile (logsDir <> "/ekg.json")
    let ekgLines = BS8.lines ekgContent

    let checkEkg name val =
          any (\line -> case decode line of
                          Just (obj :: Object) ->
                            case KeyMap.lookup "name" obj of
                              Just (String n) | n == name ->
                                case KeyMap.lookup "value" obj of
                                  Just valObj ->
                                     -- valObj is {"type": "Gauge", "val": 123}
                                     case valObj of
                                       Object v ->
                                         case KeyMap.lookup "val" v of
                                            Just (Number x) -> show x == show val
                                            _ -> False
                                       _ -> False
                                  _ -> False
                              _ -> False
                          _ -> False
              ) ekgLines

    let ekgPass = checkEkg "test.gauge" (123.0 :: Double)

    -- Check Datapoints
    dpContent <- BSL.readFile (logsDir <> "/datapoints.json")
    let dpLines = BS8.lines dpContent

    let checkDp name =
          any (\line -> case decode line of
                          Just (obj :: Object) ->
                            case KeyMap.lookup "name" obj of
                              Just (String n) | n == name -> True
                              _ -> False
                          _ -> False
              ) dpLines

    let dpPass = checkDp "test.datapoint"

    if ekgPass && dpPass
      then pure $ property True
      else do
        putStrLn $ "EKG Pass: " <> show ekgPass
        putStrLn $ "DP Pass: " <> show dpPass
        pure $ property False
